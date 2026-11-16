//! Finding a tab's YouTube video: search YouTube for the song, then score every result against
//! the tab (title, artist, channel, length, version) and rank them.
//!
//! - [`parse`] reads YouTube's search results (the web client's JSON).
//! - [`client`] fetches them ([`Searcher`] is the seam tests replace).
//! - [`matching`] scores a result against a [`Song`].
//! - [`finder`] works through a queue of tabs in the background, attaching confident matches.

pub(crate) mod client;
pub(crate) mod finder;
pub(crate) mod matching;
pub mod parse;
#[cfg(test)]
mod tests;
pub(crate) mod text;

use serde::{Deserialize, Serialize};

pub use client::{Searcher, YouTubeClient};
pub use matching::{Song, rank};

/// What a channel's badge says about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, uniffi::Enum)]
pub enum ChannelBadge {
    #[default]
    None,
    /// The grey tick.
    Verified,
    /// The music note: an Official Artist Channel.
    OfficialArtist,
}

/// One video from a search, as YouTube described it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Record)]
pub struct VideoCandidate {
    /// The 11-character video id.
    pub youtube_id: String,
    pub title: String,
    pub channel: String,
    /// `None` when YouTube didn't say (rare; live streams are dropped).
    pub duration_secs: Option<f64>,
    pub views: Option<u64>,
    pub badge: ChannelBadge,
    /// The start of the description, as shown under the result.
    pub description: String,
    /// 0-based position in YouTube's own ranking.
    pub rank: u32,
}

/// How sure the matcher is that a video is the tab's song.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, uniffi::Enum,
)]
pub enum MatchConfidence {
    Low,
    /// Probably the song; worth suggesting.
    Medium,
    /// Safe to attach without asking.
    High,
}

/// A search result scored against a tab.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, uniffi::Record)]
pub struct VideoMatch {
    pub candidate: VideoCandidate,
    /// 0–1; results are ordered by it.
    pub score: f64,
    pub confidence: MatchConfidence,
    /// The label to attach it under: "Original", "Live", "Cover", "Lesson" …
    pub label: String,
    /// Short explanations, most important first: "Official audio", "0:12 longer than the tab" …
    pub reasons: Vec<String>,
    /// Video length minus tab length, when both are known.
    pub length_difference_secs: Option<f64>,
    /// From the artist's own channel (official channel, "- Topic", VEVO).
    pub official: bool,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum SearchError {
    #[error("couldn't reach YouTube: {0}")]
    Network(String),
    #[error("YouTube is limiting requests; try again later")]
    RateLimited,
    #[error("YouTube sent a response Tabster doesn't understand")]
    Unrecognised,
}

/// Search YouTube for `song` and rank the results best first. Unless the best is a confident match
/// from the artist's own channel, a second query asks for the official audio by name.
pub fn find(searcher: &dyn Searcher, song: &Song) -> Result<Vec<VideoMatch>, SearchError> {
    let query = song.query();
    let mut candidates = searcher.search(&query)?;
    let mut ranked = matching::rank(song, &candidates);
    let official = ranked
        .first()
        .is_some_and(|m| m.confidence == MatchConfidence::High && m.official);
    if !official && song.has_artist() {
        // Official uploads sometimes rank below fan uploads; ask for them by name.
        match searcher.search(&format!("{query} official audio")) {
            Ok(more) => {
                let offset = candidates.len() as u32;
                for mut c in more {
                    if candidates.iter().all(|old| old.youtube_id != c.youtube_id) {
                        // Behind the first query's results in YouTube's ranking.
                        c.rank += offset;
                        candidates.push(c);
                    }
                }
                ranked = matching::rank(song, &candidates);
            }
            Err(e) => log::info!("second video search failed, keeping the first's results: {e}"),
        }
    }
    ranked.truncate(MAX_RESULTS);
    Ok(ranked)
}

/// Results kept per search.
pub(crate) const MAX_RESULTS: usize = 12;
