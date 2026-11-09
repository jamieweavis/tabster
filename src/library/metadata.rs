//! Catalogue metadata derived from a parsed score.

use std::path::Path;

use crate::model::Score;

/// What the `tabs` row stores about a score.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct TabMeta {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub tuning: String,
    pub track_count: u32,
    pub bar_count: u32,
    pub duration_secs: f64,
}

impl TabMeta {
    /// `path` is the imported file; its stem stands in for a missing title.
    pub fn new(score: &Score, path: &Path) -> Self {
        let title = match score.title.trim() {
            "" => path
                .file_stem()
                .map(|s| s.to_string_lossy().trim().to_string())
                .unwrap_or_default(),
            title => title.to_string(),
        };
        Self {
            title,
            artist: score.artist.trim().to_string(),
            album: score.album.trim().to_string(),
            tuning: tuning_label(score),
            track_count: score.tracks.len() as u32,
            bar_count: score.master_bars.len() as u32,
            duration_secs: estimate_duration(score),
        }
    }
}

/// Well-known tunings, open-string MIDI pitches from the lowest string up.
const NAMED_TUNINGS: &[(&str, &[u8])] = &[
    // Six-string guitar.
    ("Standard", &[40, 45, 50, 55, 59, 64]),
    ("Drop D", &[38, 45, 50, 55, 59, 64]),
    ("Eb Standard", &[39, 44, 49, 54, 58, 63]),
    ("Drop C#", &[37, 44, 49, 54, 58, 63]),
    ("D Standard", &[38, 43, 48, 53, 57, 62]),
    ("Drop C", &[36, 43, 48, 53, 57, 62]),
    ("C# Standard", &[37, 42, 47, 52, 56, 61]),
    ("Drop B", &[35, 42, 47, 52, 56, 61]),
    ("C Standard", &[36, 41, 46, 51, 55, 60]),
    ("Drop A#", &[34, 41, 46, 51, 55, 60]),
    ("B Standard", &[35, 40, 45, 50, 54, 59]),
    ("Drop A", &[33, 40, 45, 50, 54, 59]),
    ("Open G", &[38, 43, 50, 55, 59, 62]),
    ("Open D", &[38, 45, 50, 54, 57, 62]),
    ("Open E", &[40, 47, 52, 56, 59, 64]),
    ("Open A", &[40, 45, 52, 57, 61, 64]),
    ("Open C", &[36, 43, 48, 55, 60, 64]),
    ("DADGAD", &[38, 45, 50, 55, 57, 62]),
    // Seven-string guitar.
    ("Standard", &[35, 40, 45, 50, 55, 59, 64]),
    ("Drop A", &[33, 40, 45, 50, 55, 59, 64]),
    // Bass.
    ("Standard", &[28, 33, 38, 43]),
    ("Drop D", &[26, 33, 38, 43]),
    ("Standard", &[23, 28, 33, 38, 43]),
];

/// The tuning of the first non-percussion track: a common name ("Drop D") when recognised,
/// else its note names from the lowest string up ("D A D G B E"). Empty when there's no such
/// track.
pub(super) fn tuning_label(score: &Score) -> String {
    let Some(track) = score
        .tracks
        .iter()
        .find(|t| !t.is_percussion && !t.tuning.is_empty())
    else {
        return String::new();
    };
    // `tuning[0]` is string 1, the highest; labels read low to high.
    let low_to_high: Vec<u8> = track.tuning.iter().rev().copied().collect();
    if let Some((name, _)) = NAMED_TUNINGS
        .iter()
        .find(|(_, pitches)| *pitches == low_to_high.as_slice())
    {
        return (*name).to_string();
    }
    low_to_high
        .iter()
        .map(|&p| crate::model::note_name(p))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Length in seconds as played: repeats and jumps unrolled, tempo changes applied.
pub(super) fn estimate_duration(score: &Score) -> f64 {
    crate::timeline::Timeline::new(score).duration_secs()
}
