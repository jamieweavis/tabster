//! Render a tab's audio to a 16-bit stereo WAV with the bundled SoundFont:
//! `cargo run --release --example render_wav -- <tab file> <out.wav> [seconds] [soundfont]`
//! Prints peak and RMS levels.

use std::io::Write;
use std::path::Path;

use tabster_core::playback::{SAMPLE_RATE, TabSynth};
use tabster_core::timeline::Timeline;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let [_, path, out, rest @ ..] = args.as_slice() else {
        eprintln!("usage: render_wav <tab file> <out.wav> [seconds] [soundfont]");
        std::process::exit(2);
    };
    let bytes = std::fs::read(path).expect("read file");
    let ext = Path::new(path).extension().and_then(|e| e.to_str());
    let score = tabster_core::formats::parse(&bytes, ext).expect("parse");
    let timeline = Timeline::new(&score);
    let seconds: f64 = rest
        .first()
        .map_or(timeline.duration_secs(), |s| s.parse().expect("seconds"));
    let soundfont = rest.get(1).map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("Resources/Bundle/SoundFont.sf2"),
        Into::into,
    );

    let synth = TabSynth::new(&score, &timeline, &soundfont).expect("load SoundFont");
    synth.set_playing(true);
    let frames = (seconds * f64::from(SAMPLE_RATE)) as usize;
    let mut audio = vec![0.0f32; frames * 2];
    let start = std::time::Instant::now();
    for block in audio.chunks_mut(1024) {
        synth.render_into(block);
    }
    let elapsed = start.elapsed();

    let peak = audio.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    let rms =
        (audio.iter().map(|v| f64::from(v * v)).sum::<f64>() / audio.len().max(1) as f64).sqrt();
    let clipped = audio.iter().filter(|v| v.abs() >= 1.0).count();
    eprintln!(
        "{seconds:.1} s rendered in {:.0} ms; peak {peak:.3} ({:.1} dBFS), RMS {rms:.4} ({:.1} dBFS), {clipped} clipped samples",
        elapsed.as_secs_f64() * 1000.0,
        20.0 * peak.max(1e-9).log10(),
        20.0 * rms.max(1e-9).log10(),
    );

    let data_len = (audio.len() * 2) as u32;
    let mut wav = Vec::with_capacity(44 + data_len as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&2u16.to_le_bytes()); // channels
    wav.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    wav.extend_from_slice(&(SAMPLE_RATE * 4).to_le_bytes()); // byte rate
    wav.extend_from_slice(&4u16.to_le_bytes()); // block align
    wav.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    for v in &audio {
        let s = (v.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16;
        wav.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::File::create(out)
        .and_then(|mut f| f.write_all(&wav))
        .expect("write wav");
}
