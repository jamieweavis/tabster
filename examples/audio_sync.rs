//! Align a recording to a tab and print where every bar falls (the automatic-sync spike):
//! `cargo run --release --example audio_sync -- <tab file> <recording.wav> [video secs of the wav's start]`
//!
//! The WAV must be uncompressed PCM (16-bit or 32-bit float); channels are mixed to mono.

use std::path::Path;

use tabster_core::audio_sync::{AudioCapture, sync_from_capture};
use tabster_core::sync::SyncSpec;
use tabster_core::timeline::Timeline;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let [_, tab, wav, rest @ ..] = args.as_slice() else {
        eprintln!("usage: audio_sync <tab file> <recording.wav> [video secs of the wav's start]");
        std::process::exit(2);
    };
    let video_start: f64 = rest.first().map_or(0.0, |s| s.parse().expect("seconds"));

    let bytes = std::fs::read(tab).expect("read tab");
    let ext = Path::new(tab).extension().and_then(|e| e.to_str());
    let score = tabster_core::formats::parse(&bytes, ext).expect("parse tab");
    let timeline = Timeline::new(&score);

    let (rate, samples) =
        read_wav(&std::fs::read(wav).expect("read wav")).expect("unsupported WAV");
    let capture = AudioCapture::new(rate);
    let chunk = (rate as usize / 2).max(1);
    for (i, block) in samples.chunks(chunk).enumerate() {
        capture.push_samples(block, Some(video_start + (i * chunk) as f64 / rate));
    }

    let start = std::time::Instant::now();
    let found = match sync_from_capture(&score, &timeline, &capture) {
        Ok(found) => found,
        Err(e) => {
            eprintln!("no sync: {e}");
            std::process::exit(1);
        }
    };
    eprintln!(
        "{:.1} s of audio aligned in {:.0} ms: bars {}–{} of {}, {} matched by onsets, confidence {:.2}, similarity {:.3}, stretch {:.3}, transpose {:+}",
        capture.duration_secs(),
        start.elapsed().as_secs_f64() * 1000.0,
        found.first_bar + 1,
        found.last_bar + 1,
        timeline.played_bars().len(),
        found.matched_bars,
        found.confidence,
        found.similarity,
        found.stretch,
        found.transpose,
    );
    let SyncSpec::Points { points } = found.sync else {
        return;
    };
    // The same JSON the app's "Paste Per-Bar Points…" takes.
    let times: Vec<String> = points
        .iter()
        .map(|p| format!("{:.3}", p.video_secs))
        .collect();
    println!("[{}]", times.join(", "));
}

/// Mono samples and the sample rate of a PCM WAV.
fn read_wav(bytes: &[u8]) -> Option<(f64, Vec<f32>)> {
    if bytes.get(..4)? != b"RIFF" || bytes.get(8..12)? != b"WAVE" {
        return None;
    }
    let u16_at = |at: usize| Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?));
    let u32_at = |at: usize| Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?));
    let (mut format, mut channels, mut rate, mut bits) = (0u16, 0usize, 0u32, 0u16);
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32_at(at + 4)? as usize;
        let body = at + 8;
        if id == b"fmt " {
            format = u16_at(body)?;
            channels = usize::from(u16_at(body + 2)?);
            rate = u32_at(body + 4)?;
            bits = u16_at(body + 14)?;
            // WAVE_FORMAT_EXTENSIBLE: the real format is the sub-format's first two bytes.
            if format == 0xFFFE {
                format = u16_at(body + 24)?;
            }
        } else if id == b"data" {
            let data = bytes.get(body..(body + size).min(bytes.len()))?;
            let width = usize::from(bits / 8);
            if channels == 0 || width == 0 {
                return None;
            }
            let sample = |b: &[u8]| -> Option<f32> {
                match (format, bits) {
                    (1, 16) => Some(f32::from(i16::from_le_bytes([b[0], b[1]])) / 32_768.0),
                    (1, 24) => {
                        Some((i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) as f32 / 8_388_608.0)
                    }
                    (3, 32) => Some(f32::from_le_bytes([b[0], b[1], b[2], b[3]])),
                    _ => None,
                }
            };
            let mut mono = Vec::with_capacity(data.len() / (width * channels));
            for frame in data.chunks_exact(width * channels) {
                let mut sum = 0.0;
                for b in frame.chunks_exact(width) {
                    sum += sample(b)?;
                }
                mono.push(sum / channels as f32);
            }
            return Some((f64::from(rate), mono));
        }
        at = body + size + (size & 1);
    }
    None
}
