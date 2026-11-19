//! The tab's side of the alignment: the same features as [`super::features`], predicted from
//! the score's note events instead of measured from audio.

use std::collections::HashMap;

use super::features::{CHROMA_RATE, Features, ONSET_RATE};
use crate::playback::events::{Events, PERCUSSION_CHANNEL};

/// How fast a plucked note's contribution to the chroma dies away (seconds).
const NOTE_DECAY_SECS: f64 = 1.5;
/// Partials above the fundamental that land on other pitch classes: (semitones, level).
const PARTIALS: [(u8, f32); 2] = [(7, 0.35), (4, 0.15)];
/// Onsets of pitched notes count a little less than drum hits.
const PITCHED_ONSET: f32 = 0.7;
/// Notes below this key (C3) are the bass register.
const BASS_BELOW_KEY: u8 = 48;

/// Predict the features of a recording of `events`, on the score-seconds axis.
pub fn reference(events: &Events) -> Features {
    let duration = events.duration_secs.max(0.0);
    let mut onset = vec![0.0f32; (duration * ONSET_RATE).ceil() as usize + 1];
    let mut chroma = vec![[0.0f32; 12]; (duration * CHROMA_RATE).ceil() as usize + 1];
    let mut bass = chroma.clone();

    // Note-ons waiting for their note-off, per (track, channel, key).
    let mut sounding: HashMap<(u16, u8, u8), Vec<(f64, u8)>> = HashMap::new();
    for e in &events.notes {
        let id = (e.track, e.channel, e.key);
        if e.is_on() {
            let weight = f32::from(e.velocity) / 127.0;
            let weight = if e.channel == PERCUSSION_CHANNEL {
                weight
            } else {
                weight * PITCHED_ONSET
            };
            splat(&mut onset, e.secs * ONSET_RATE, weight);
            sounding.entry(id).or_default().push((e.secs, e.velocity));
        } else if let Some((start, velocity)) = sounding.get_mut(&id).and_then(Vec::pop)
            && e.channel != PERCUSSION_CHANNEL
        {
            add_note(&mut chroma, start, e.secs, e.key, velocity);
            if e.key < BASS_BELOW_KEY {
                add_note(&mut bass, start, e.secs, e.key, velocity);
            }
        }
    }
    // Notes never released ring to the end.
    for ((_, channel, key), starts) in sounding {
        if channel == PERCUSSION_CHANNEL {
            continue;
        }
        for (start, velocity) in starts {
            add_note(&mut chroma, start, duration, key, velocity);
            if key < BASS_BELOW_KEY {
                add_note(&mut bass, start, duration, key, velocity);
            }
        }
    }

    // Chords are louder than single notes, but not in proportion.
    for v in &mut onset {
        *v = (2.0 * *v).ln_1p();
    }
    Features {
        onset,
        chroma,
        bass,
    }
}

/// Add `weight` at fractional frame `position`, split between its two neighbours.
fn splat(frames: &mut [f32], position: f64, weight: f32) {
    if !position.is_finite() || position < 0.0 {
        return;
    }
    let i = position.floor() as usize;
    let frac = (position - position.floor()) as f32;
    if let Some(v) = frames.get_mut(i) {
        *v += weight * (1.0 - frac);
    }
    if let Some(v) = frames.get_mut(i + 1) {
        *v += weight * frac;
    }
}

fn add_note(chroma: &mut [[f32; 12]], start: f64, end: f64, key: u8, velocity: u8) {
    if end.partial_cmp(&start) != Some(std::cmp::Ordering::Greater) {
        return;
    }
    let level = f32::from(velocity) / 127.0;
    let first = (start * CHROMA_RATE - 0.5).ceil().max(0.0) as usize;
    let last = ((end * CHROMA_RATE + 0.5).floor() as usize).min(chroma.len().saturating_sub(1));
    for (f, frame) in chroma.iter_mut().enumerate().take(last + 1).skip(first) {
        // How much of the frame's span the note sounds for.
        let lo = (f as f64 - 0.5) / CHROMA_RATE;
        let hi = (f as f64 + 0.5) / CHROMA_RATE;
        let overlap = ((hi.min(end) - lo.max(start)) * CHROMA_RATE).clamp(0.0, 1.0);
        let age = (f as f64 / CHROMA_RATE - start).max(0.0);
        let w = level * (overlap * (-age / NOTE_DECAY_SECS).exp()) as f32;
        frame[usize::from(key % 12)] += w;
        for (interval, partial) in PARTIALS {
            frame[usize::from((key + interval) % 12)] += w * partial;
        }
    }
}
