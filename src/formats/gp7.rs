//! Guitar Pro 7/8 `.gp`: a zip containing `Content/score.gpif`.
//! Owner: Phase 1 agent A.

use std::io::{Cursor, Read};

use super::ParseError;
use super::gpif::parse_gpif;
use crate::model::Score;

const SCORE_PATH: &str = "Content/score.gpif";
/// Refuse to inflate a score larger than this (zip-bomb guard; the GPIF DOM costs a few times
/// the document's size). Real scores are a few MB. Same limit as GPX.
const MAX_SCORE_BYTES: u64 = 64 * 1024 * 1024;

pub fn parse(bytes: &[u8]) -> Result<Score, ParseError> {
    parse_gpif(&extract_score(bytes)?)
}

/// Pull `Content/score.gpif` out of a `.gp` zip.
pub fn extract_score(bytes: &[u8]) -> Result<Vec<u8>, ParseError> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|e| ParseError::Container(format!("not a valid .gp zip: {e}")))?;
    let name = if zip.index_for_name(SCORE_PATH).is_some() {
        SCORE_PATH.to_owned()
    } else {
        // Be lenient about case and directory layout.
        zip.file_names()
            .filter_map(Result::ok)
            .find(|n| n.to_ascii_lowercase().ends_with("score.gpif"))
            .map(|n| n.into_owned())
            .ok_or_else(|| ParseError::Container(format!("{SCORE_PATH} not found")))?
    };
    let file = zip
        .by_name(&name)
        .map_err(|e| ParseError::Container(format!("can't open {name}: {e}")))?;
    // The entry's size is the file's claim: don't preallocate more than the input could hold.
    let plausible = (bytes.len() as u64).saturating_mul(32);
    let mut xml = Vec::with_capacity(file.size().min(MAX_SCORE_BYTES).min(plausible) as usize);
    file.take(MAX_SCORE_BYTES + 1)
        .read_to_end(&mut xml)
        .map_err(|e| ParseError::Container(format!("can't read {name}: {e}")))?;
    if xml.len() as u64 > MAX_SCORE_BYTES {
        return Err(ParseError::Container(format!("{name} is too large")));
    }
    Ok(xml)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use zip::write::SimpleFileOptions;

    use super::*;
    use crate::formats::{self, Format};

    const SHOWCASE: &[u8] = include_bytes!("../../testdata/gp7/showcase.gpif");

    /// Build a `.gp`-like zip from (path, contents) entries.
    fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (path, data) in entries {
            w.start_file(*path, opts).unwrap();
            w.write_all(data).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    fn showcase_gp() -> Vec<u8> {
        zip_of(&[
            ("VERSION", b"7.0"),
            ("meta.json", b"{}"),
            (SCORE_PATH, SHOWCASE),
        ])
    }

    #[test]
    fn parses_zipped_score_like_raw_gpif() {
        let from_zip = parse(&showcase_gp()).unwrap();
        assert_eq!(from_zip, parse_gpif(SHOWCASE).unwrap());
        assert_eq!(from_zip.title, "Tabster Showcase");
    }

    #[test]
    fn dispatch_through_formats_parse() {
        let gp = showcase_gp();
        assert_eq!(formats::detect(&gp, None), Some(Format::Gp7));
        let score = formats::parse(&gp, Some("gp")).unwrap();
        assert_eq!(score.tracks.len(), 2);
        assert_eq!(score.master_bars.len(), 8);
    }

    #[test]
    fn finds_score_with_unusual_path() {
        let gp = zip_of(&[("content/Score.GPIF", SHOWCASE)]);
        assert_eq!(parse(&gp).unwrap().title, "Tabster Showcase");
    }

    #[test]
    fn missing_score_is_container_error() {
        let gp = zip_of(&[("VERSION", b"7.0")]);
        assert!(matches!(parse(&gp), Err(ParseError::Container(_))));
    }

    #[test]
    fn truncated_zip_is_error() {
        let gp = showcase_gp();
        for len in [0, 4, 30, gp.len() / 2, gp.len() - 10] {
            assert!(parse(&gp[..len]).is_err(), "truncated at {len}");
        }
    }

    #[test]
    fn garbage_is_error() {
        assert!(parse(b"PK\x03\x04 this is not a zip").is_err());
        let gp = zip_of(&[(SCORE_PATH, b"\x00\x01 not xml <<<")]);
        assert!(matches!(parse(&gp), Err(ParseError::Invalid(_))));
    }

    #[test]
    fn corrupted_bytes_never_panic() {
        let gp = showcase_gp();
        for i in (0..gp.len()).step_by(7) {
            let mut bad = gp.clone();
            bad[i] ^= 0x5A;
            let _ = parse(&bad);
        }
    }

    /// A real Guitar Pro 8 file from the user's machine (copyrighted: never committed or
    /// snapshotted). Run with `cargo test --release -- --ignored real_gp8`.
    #[test]
    #[ignore]
    fn real_gp8_file() {
        let path = "/Users/jamie/Developer/tabster/Men I Trust-Show Me How-09-08-2026.gp";
        let Ok(bytes) = std::fs::read(path) else {
            eprintln!("skipping: {path} not found");
            return;
        };
        let start = std::time::Instant::now();
        let score = parse(&bytes).unwrap();
        let elapsed = start.elapsed();
        eprintln!("parsed in {elapsed:?}");
        assert_eq!(score.master_bars.len(), 145);
        assert!(score.master_bars.iter().all(|mb| mb.time_signature
            == crate::model::TimeSignature {
                numerator: 4,
                denominator: 4
            }));
        assert_eq!(score.initial_tempo, 166.0);
        assert_eq!(score.tempo_changes.len(), 1);
        assert!(score.sync_points.is_empty());
        assert!(
            score
                .master_bars
                .iter()
                .all(|mb| !mb.repeat_start && mb.repeat_end == 0)
        );
        assert_eq!(score.tracks.len(), 5);
        for t in &score.tracks {
            assert_eq!(t.bars.len(), 145, "{}", t.name);
        }
        assert!(score.tracks[0].name.ends_with("Lead Vocals"));
        assert_eq!(score.tracks[1].tuning, [64, 59, 55, 50, 45, 40]);
        assert_eq!(score.tracks[2].tuning, [64, 59, 55, 50, 45, 40]);
        assert_eq!(score.tracks[3].tuning, [43, 38, 33, 28]);
        assert!(score.tracks[4].is_percussion);
        assert!(score.tracks[..4].iter().all(|t| !t.is_percussion));
        let sections: Vec<(usize, &str)> = score
            .master_bars
            .iter()
            .enumerate()
            .filter_map(|(i, mb)| mb.section.as_ref().map(|s| (i, s.text.as_str())))
            .collect();
        assert_eq!(
            sections,
            [
                (1, "Intro"),
                (17, "Verse 1"),
                (49, "Chorus"),
                (65, "Post-Chorus"),
                (73, "Verse 2"),
                (105, "Chorus"),
                (121, "Post-Chorus"),
                (129, "Outro")
            ]
        );
        for t in &score.tracks {
            let notes = t
                .bars
                .iter()
                .flat_map(|b| &b.voices)
                .flat_map(|v| &v.beats)
                .flat_map(|b| &b.notes);
            let mut count = 0;
            for n in notes {
                count += 1;
                assert!(t.pitch(n.string, n.fret).is_some(), "{}: {n:?}", t.name);
            }
            eprintln!("{}: {count} notes", t.name);
        }
        // Each bar's primary voice should fill the 4/4 bar (or be empty). Grace beats take no
        // time in Guitar Pro, but `Voice::ticks` counts them, so leave them out here.
        let mut graces = 0;
        for t in &score.tracks {
            for (i, b) in t.bars.iter().enumerate() {
                let beats = b.voices.first().map_or(&[][..], |v| &v.beats[..]);
                graces += beats.iter().filter(|b| b.effects.grace).count();
                let ticks: u32 = beats
                    .iter()
                    .filter(|b| !b.effects.grace)
                    .map(|b| b.duration.ticks())
                    .sum();
                assert!(
                    ticks == 0 || ticks == 3840,
                    "{} bar {i}: {ticks} ticks",
                    t.name
                );
            }
        }
        eprintln!("{graces} grace beats");
        if !cfg!(debug_assertions) {
            assert!(elapsed.as_millis() < 20, "too slow: {elapsed:?}");
        }
    }
}
