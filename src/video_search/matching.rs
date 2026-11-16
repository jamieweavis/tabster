//! Scoring YouTube results against a tab.
//!
//! Each result gets a 0–1 score from:
//! - **title**: how many of the song's words the video title has (typos allowed), in order;
//! - **artist**: the artist in the channel name or the title;
//! - **length**: the video's length against the tab's. Shorter is penalised hard (the tab covers
//!   the whole song); longer only gently (music videos have intros and outros);
//! - **channel**: the artist's own channel, its "- Topic" channel or an auto-generated
//!   "Provided to YouTube by" upload (studio audio, the best thing to sync to);
//! - **version**: covers, lessons, live recordings, remixes, reactions … are penalised unless the
//!   tab's own title asks for that version ("Song (Live)");
//! - popularity and YouTube's own rank, as tiebreakers.
//!
//! [`MatchConfidence::High`] is only given when title, artist and length all agree and nothing
//! suggests another version, since a High match is attached without asking.

use std::cmp::Ordering;

use super::text::{
    compact, contains_phrase, format_clock, is_stop_word, split_brackets, tokens,
    without_featuring, word_similarity,
};
use super::{ChannelBadge, MatchConfidence, VideoCandidate, VideoMatch};

/// The tab being matched, prepared for comparison.
#[derive(Debug, Clone, PartialEq)]
pub struct Song {
    /// The title as searched for (no bracketed parts, no "feat." clause).
    title: String,
    artist: String,
    /// Words of the title a match must find; stop words dropped unless that leaves none.
    title_words: Vec<String>,
    /// Every word of the title outside brackets, in order.
    title_phrase: Vec<String>,
    /// The artist's words without a leading "the" or a "feat." clause.
    artist_words: Vec<String>,
    artist_compact: String,
    /// 0 when unknown.
    duration_secs: f64,
    /// Versions the tab (or the user's search) asks for: "Song (Live)".
    wanted: Vec<Version>,
    /// The tab has no artist; `artist_words` is what most results agree it is.
    artist_inferred: bool,
}

impl Song {
    /// `context` is extra text that can ask for a version (a search the user typed).
    pub fn new(title: &str, artist: &str, duration_secs: f64, context: &str) -> Self {
        let (mut title, mut artist) = (title.trim(), artist.trim());
        if artist.is_empty() {
            // "Artist - Title" in the title (often a file name standing in for a missing title).
            if let Some((a, t)) = title.split_once(" - ")
                && !a.trim().is_empty()
                && !t.trim().is_empty()
            {
                artist = a.trim();
                title = t.trim();
            }
        } else if let Some((a, t)) = title.split_once(" - ")
            && compact(a) == compact(artist)
            && !t.trim().is_empty()
        {
            title = t.trim();
        }

        let (outside, _) = split_brackets(title);
        let mut phrase = without_featuring(&tokens(&outside)).to_vec();
        let mut display = outside.split_whitespace().collect::<Vec<_>>().join(" ");
        if phrase.is_empty() {
            // The whole title is in brackets.
            phrase = tokens(title);
            display = title.to_string();
        } else if let Some(cut) = featuring_cut(&display) {
            display.truncate(cut);
        }
        let required: Vec<String> = phrase
            .iter()
            .filter(|w| !is_stop_word(w))
            .cloned()
            .collect();
        let title_words = if required.is_empty() {
            phrase.clone()
        } else {
            required
        };

        let mut artist_words = without_featuring(&tokens(artist)).to_vec();
        if artist_words.len() > 1 && artist_words[0] == "the" {
            artist_words.remove(0);
        }
        let artist_compact = artist_words.concat();

        // Versions asked for in brackets ("Song (Live)") or in the user's search, not words of
        // the song's name ("Live Forever").
        let (_, brackets) = split_brackets(title);
        let mut asked: Vec<String> = brackets.iter().flat_map(|b| tokens(b)).collect();
        asked.extend(
            tokens(context)
                .into_iter()
                .filter(|w| !phrase.contains(w) && !artist_words.contains(w)),
        );
        let wanted = VERSIONS
            .iter()
            .filter(|v| (v.detect)(&asked))
            .map(|v| v.version)
            .collect();

        Self {
            title: display.trim().to_string(),
            artist: artist.to_string(),
            title_words,
            title_phrase: phrase,
            artist_words,
            artist_compact,
            duration_secs: if duration_secs.is_finite() {
                duration_secs.max(0.0)
            } else {
                0.0
            },
            wanted,
            artist_inferred: false,
        }
    }

    /// This song with an artist guessed from search results (the search text is unchanged).
    fn with_inferred_artist(&self, artist: &str) -> Self {
        let mut song = self.clone();
        song.artist_words = without_featuring(&tokens(artist)).to_vec();
        if song.artist_words.len() > 1 && song.artist_words[0] == "the" {
            song.artist_words.remove(0);
        }
        song.artist_compact = song.artist_words.concat();
        song.artist_inferred = true;
        song
    }

    pub fn has_artist(&self) -> bool {
        !self.artist_words.is_empty()
    }

    /// What to type into YouTube's search box.
    pub fn query(&self) -> String {
        let mut query = if self.artist.is_empty() {
            self.title.clone()
        } else {
            format!("{} {}", self.artist, self.title)
        };
        for version in &self.wanted {
            if let Some(word) = info(*version).query_word
                && !self.title_phrase.iter().any(|w| w == word)
            {
                query.push(' ');
                query.push_str(word);
            }
        }
        query
    }

    fn wants(&self, version: Version) -> bool {
        self.wanted.contains(&version)
    }
}

/// Byte offset of a " feat." / " ft." / " featuring" clause in a display title.
fn featuring_cut(title: &str) -> Option<usize> {
    let lower = title.to_lowercase();
    [" feat.", " feat ", " ft.", " ft ", " featuring "]
        .iter()
        .filter_map(|m| lower.find(m))
        .min()
        // Lowercasing can change lengths outside ASCII; only trust offsets that line up.
        .filter(|&i| title.is_char_boundary(i) && lower.len() == title.len())
}

/// Score every candidate and sort best first (YouTube's order breaks ties).
///
/// A tab without an artist is matched against the artist most results agree on ("Dire Straits -
/// Sultans of Swing" in title after title), though never with [`MatchConfidence::High`].
pub fn rank(song: &Song, candidates: &[VideoCandidate]) -> Vec<VideoMatch> {
    let inferred;
    let song = match infer_artist(song, candidates) {
        Some(artist) if !song.has_artist() => {
            inferred = song.with_inferred_artist(&artist);
            &inferred
        }
        _ => song,
    };
    let mut matches: Vec<VideoMatch> = candidates.iter().map(|c| score(song, c)).collect();
    matches.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(Ordering::Equal)
            .then(a.candidate.rank.cmp(&b.candidate.rank))
    });
    matches
}

/// Score one candidate.
pub fn score(song: &Song, c: &VideoCandidate) -> VideoMatch {
    let words = tokens(&c.title);
    let mut explained = vec![false; words.len()];

    // Title.
    let title = title_score(song, &words, &mut explained);

    // Artist and channel.
    let (channel_name, topic) = match c.channel.trim_end().strip_suffix("- Topic") {
        Some(name) => (name.trim_end(), true),
        None => (c.channel.as_str(), false),
    };
    let channel_compact = compact(channel_name);
    let channel_is_artist = song.has_artist() && is_artists_channel(song, &channel_compact);
    let artist = song.has_artist().then(|| {
        artist_score(
            song,
            &words,
            &mut explained,
            channel_is_artist,
            &c.description,
        )
    });
    let channel_words = tokens(channel_name);
    for (i, w) in words.iter().enumerate() {
        if channel_words.contains(w) {
            explained[i] = true;
        }
    }

    // Versions, from the words the song's name, artist and channel don't account for (so "Live
    // Forever" isn't a live recording).
    let art_track = c
        .description
        .trim_start()
        .starts_with("Provided to YouTube by");
    let rest: Vec<String> = words
        .iter()
        .zip(&explained)
        .filter(|(_, e)| !**e)
        .map(|(w, _)| w.clone())
        .collect();
    let mut detected: Vec<Version> = VERSIONS
        .iter()
        .filter(|v| (v.detect)(&rest))
        .map(|v| v.version)
        .collect();
    let segments = qualifier_segments(&c.title);
    let is_known = |w: &str| {
        is_neutral(w)
            || song.title_phrase.iter().any(|t| t == w)
            || song.artist_words.iter().any(|a| a == w)
            || channel_words.iter().any(|ch| ch == w)
    };
    if !detected.contains(&Version::Live)
        && (looks_like_a_date(&c.title, &words, &segments, &is_known)
            || !art_track && live_description(&tokens(&c.description)))
    {
        detected.push(Version::Live);
    }
    if alternate_take(&segments, &is_known) {
        detected.push(Version::Alternate);
    }
    for (i, w) in words.iter().enumerate() {
        if !explained[i]
            && detected
                .iter()
                .any(|v| info(*v).words.contains(&w.as_str()))
        {
            explained[i] = true;
        }
    }
    let unwanted: Vec<Version> = detected
        .iter()
        .copied()
        .filter(|v| !song.wants(*v))
        .collect();
    let matched_wanted = detected.iter().any(|v| song.wants(*v));

    // Words the song, artist, channel and versions don't account for: another song, a medley,
    // "on one guitar only" …
    let extra = words
        .iter()
        .zip(&explained)
        .filter(|(w, explained)| !**explained && !is_neutral(w))
        .count();

    // Length.
    let length = match (c.duration_secs, song.duration_secs) {
        (Some(video), tab) if tab >= MIN_TAB_SECS && video > 0.0 => Some(length_score(tab, video)),
        _ => None,
    };
    let length_difference_secs = c
        .duration_secs
        .filter(|_| song.duration_secs >= MIN_TAB_SECS)
        .map(|v| v - song.duration_secs);

    // Combine the matching signals by weight, gated on the title: a perfect artist and length
    // can't make up for the wrong song.
    let mut weighted = TITLE_WEIGHT * title;
    let mut total = TITLE_WEIGHT;
    if let Some(a) = artist {
        weighted += ARTIST_WEIGHT * a;
        total += ARTIST_WEIGHT;
    }
    if let Some(l) = length {
        weighted += LENGTH_WEIGHT * l;
        total += LENGTH_WEIGHT;
    }
    let mut base = weighted / total;
    if title < TITLE_GATE {
        base *= (title / TITLE_GATE).powi(2);
    }

    let mut reasons = Vec::new();
    let channel_bonus = if channel_is_artist && (topic || art_track) {
        reasons.push("Official audio".to_string());
        0.12
    } else if channel_is_artist && c.badge == ChannelBadge::OfficialArtist {
        reasons.push("Artist's channel".to_string());
        0.07
    } else if channel_is_artist && channel_compact.ends_with("vevo") {
        reasons.push("Artist's channel".to_string());
        0.06
    } else if channel_is_artist {
        reasons.push(
            if c.badge == ChannelBadge::Verified {
                "Verified channel"
            } else {
                "Artist's channel"
            }
            .to_string(),
        );
        if c.badge == ChannelBadge::Verified {
            0.04
        } else {
            0.03
        }
    } else {
        0.0
    };
    // "Official" only means something from the artist's own channel.
    let official = if !channel_is_artist {
        0.0
    } else if contains_phrase(&words, &["official", "audio"]) {
        0.02
    } else if contains_phrase(&words, &["official"]) {
        0.01
    } else {
        0.0
    };
    let popularity = c.views.map_or(0.0, |v| {
        POPULARITY_WEIGHT * ((v as f64 + 1.0).log10() / 8.0).min(1.0)
    });
    let position = RANK_WEIGHT * (1.0 - f64::from(c.rank) / 20.0).max(0.0);
    let wanted_bonus = if matched_wanted { 0.04 } else { 0.0 };

    let version_penalty: f64 = unwanted
        .iter()
        .map(|v| info(*v).penalty)
        .sum::<f64>()
        .min(0.8);
    let extra_penalty = (EXTRA_WORD_PENALTY * extra.saturating_sub(1) as f64).min(0.2);
    // A clip or a medley: the right song, but not something to play the tab along with.
    let length_penalty = length.map_or(0.0, |l| (0.3 * (1.0 - l / 0.3)).max(0.0));

    let score = (0.85 * base + channel_bonus + official + popularity + position + wanted_bonus
        - version_penalty
        - extra_penalty
        - length_penalty)
        .clamp(0.0, 1.0);

    if let Some(difference) = length_difference_secs {
        let tolerance = (0.025 * song.duration_secs).max(4.0);
        reasons.push(if difference.abs() <= tolerance {
            "Same length as the tab".to_string()
        } else if difference > 0.0 {
            format!("{} longer than the tab", format_clock(difference))
        } else {
            format!("{} shorter than the tab", format_clock(difference))
        });
    }
    for v in &detected {
        let reason = info(*v).reason;
        if !reasons.iter().any(|r| r == reason) {
            reasons.push(reason.to_string());
        }
    }
    if title < TITLE_GATE {
        reasons.push("Different title".to_string());
    }
    if artist.is_some_and(|a| a < 0.5) {
        reasons.push("Artist not mentioned".to_string());
    }

    let confidence = if score >= 0.80
        && title >= 0.85
        && artist.is_some_and(|a| a >= 0.85)
        && !song.artist_inferred
        && length.is_some_and(|l| l >= 0.5)
        && unwanted.is_empty()
        && extra <= 2
    {
        MatchConfidence::High
    } else if score >= 0.55 && title >= TITLE_GATE && length.is_none_or(|l| l >= 0.25) {
        MatchConfidence::Medium
    } else {
        MatchConfidence::Low
    };

    let label = detected
        .iter()
        .filter_map(|v| info(*v).label)
        .next()
        .unwrap_or("Original")
        .to_string();

    VideoMatch {
        candidate: c.clone(),
        score,
        confidence,
        label,
        reasons,
        length_difference_secs,
        official: channel_is_artist,
    }
}

const TITLE_WEIGHT: f64 = 0.45;
const ARTIST_WEIGHT: f64 = 0.30;
const LENGTH_WEIGHT: f64 = 0.25;
/// Below this title score the result is probably another song.
const TITLE_GATE: f64 = 0.6;
const POPULARITY_WEIGHT: f64 = 0.03;
const RANK_WEIGHT: f64 = 0.02;
/// Per unexplained word in the video title, after the first.
const EXTRA_WORD_PENALTY: f64 = 0.035;
/// Tabs shorter than this have no useful length (an empty or one-bar file).
const MIN_TAB_SECS: f64 = 20.0;

/// How many of the song's words the title has (0–1), allowing typos and words run together;
/// marks the title words used in `explained`. The whole title as a phrase scores 1.
fn title_score(song: &Song, words: &[String], explained: &mut [bool]) -> f64 {
    if song.title_words.is_empty() {
        return 0.0;
    }
    // The whole title, stop words and all, as a phrase.
    let phrase: Vec<&str> = song.title_phrase.iter().map(String::as_str).collect();
    if !phrase.is_empty()
        && let Some(start) = words
            .windows(phrase.len())
            .position(|w| w.iter().zip(&phrase).all(|(a, b)| a == b))
    {
        explained[start..start + phrase.len()]
            .iter_mut()
            .for_each(|e| *e = true);
        return 1.0;
    }
    let mut total = 0.0;
    let mut positions = Vec::new();
    for want in &song.title_words {
        let mut best = (0.0, None);
        for (i, w) in words.iter().enumerate() {
            let s = word_similarity(want, w);
            if s > best.0 {
                best = (s, Some((i, 1)));
            }
            // "Wonder Wall" for "Wonderwall".
            if let Some(next) = words.get(i + 1)
                && want.len() == w.len() + next.len()
                && want.starts_with(w.as_str())
                && want.ends_with(next.as_str())
            {
                best = (1.0, Some((i, 2)));
            }
            if best.0 == 1.0 {
                break;
            }
        }
        if let (s, Some((i, n))) = best {
            total += s;
            positions.push(i);
            explained[i..i + n].iter_mut().for_each(|e| *e = true);
        }
    }
    let coverage = total / song.title_words.len() as f64;
    let in_order = positions.windows(2).all(|p| p[0] <= p[1]);
    if in_order { coverage } else { coverage * 0.92 }
}

/// The channel is the artist's: "Metallica", "MetallicaTV", "TOTOVEVO", "The Beatles - Topic".
fn is_artists_channel(song: &Song, channel: &str) -> bool {
    if channel.is_empty() || song.artist_compact.is_empty() {
        return false;
    }
    let mut stripped = channel.strip_prefix("the").unwrap_or(channel);
    stripped = stripped.strip_prefix("official").unwrap_or(stripped);
    loop {
        let before = stripped;
        for suffix in [
            "vevo", "official", "music", "band", "tv", "channel", "hq", "topic",
        ] {
            if let Some(s) = stripped.strip_suffix(suffix)
                && !s.is_empty()
            {
                stripped = s;
            }
        }
        if stripped == before {
            break;
        }
    }
    let artist = song.artist_compact.as_str();
    stripped == artist || (artist.len() >= 5 && stripped.contains(artist))
}

/// How clearly the result names the artist (0–1); marks the artist's words in `explained`.
fn artist_score(
    song: &Song,
    words: &[String],
    explained: &mut [bool],
    channel_is_artist: bool,
    description: &str,
) -> f64 {
    let artist: Vec<&str> = song.artist_words.iter().map(String::as_str).collect();
    let mut in_title = false;
    if let Some(start) = words
        .windows(artist.len())
        .position(|w| w.iter().zip(&artist).all(|(a, b)| a == b))
    {
        explained[start..start + artist.len()]
            .iter_mut()
            .for_each(|e| *e = true);
        in_title = true;
    } else if words.contains(&song.artist_compact) {
        // "ACDC" for "AC/DC" → "ac dc".
        for (i, w) in words.iter().enumerate() {
            if *w == song.artist_compact {
                explained[i] = true;
            }
        }
        in_title = true;
    }
    if channel_is_artist {
        return 1.0;
    }
    if in_title {
        return 0.9;
    }
    // Some of the artist's words, typos allowed.
    let found: f64 = artist
        .iter()
        .map(|a| {
            words
                .iter()
                .map(|w| word_similarity(a, w))
                .fold(0.0, f64::max)
        })
        .sum::<f64>()
        / artist.len() as f64;
    let mut score = 0.8 * found;
    if score < 0.5 && contains_phrase(&tokens(description), &artist) {
        // "Artist: Polyphia Song: Playing God" in the description.
        score = 0.5;
    }
    score
}

/// 0–1. Shorter than the tab falls off fast; longer slowly, since videos add intros and outros
/// and transcriptions often skip a fade-out. Wildly different lengths score 0.
fn length_score(tab: f64, video: f64) -> f64 {
    if video > 3.0 * tab || video < tab / 3.0 {
        return 0.0;
    }
    let difference = video - tab;
    let sigma = if difference >= 0.0 {
        15.0 + 0.10 * tab
    } else {
        6.0 + 0.05 * tab
    };
    (-0.5 * (difference / sigma).powi(2)).exp()
}

/// Words that don't make a title another song's.
fn is_neutral(word: &str) -> bool {
    is_stop_word(word)
        || is_year(word)
        || matches!(
            word,
            "official"
                | "video"
                | "music"
                | "audio"
                | "lyric"
                | "lyrics"
                | "hd"
                | "hq"
                | "4k"
                | "8k"
                | "1080p"
                | "720p"
                | "60fps"
                | "remaster"
                | "remastered"
                | "remastering"
                | "version"
                | "edit"
                | "single"
                | "album"
                | "ft"
                | "feat"
                | "featuring"
                | "mv"
                | "visualizer"
                | "visualiser"
                | "explicit"
                | "clean"
                | "clip"
                | "original"
                | "by"
                | "from"
                | "with"
                | "song"
                | "track"
                | "stereo"
                | "mono"
                | "videoclip"
                | "officiel"
                | "oficial"
                | "uncensored"
                | "quality"
                | "high"
                | "new"
                | "studio"
                | "radio"
                | "extended"
                | "deluxe"
                | "edition"
                | "anniversary"
                | "bonus"
                | "mix"
                | "full"
                | "upgrade"
                | "restored"
                | "remastered4k"
                | "w"
                | "x"
                | "vs"
                | "lp"
                | "ep"
                | "vinyl"
                | "cd"
                | "mp3"
                | "flac"
                | "kbps"
                | "320"
        )
}

fn is_year(word: &str) -> bool {
    word.len() == 4
        && word.bytes().all(|b| b.is_ascii_digit())
        && (word.starts_with("19") || word.starts_with("20"))
}

const MONTHS: &[&str] = &[
    "january",
    "february",
    "march",
    "april",
    "may",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
    "jan",
    "feb",
    "mar",
    "apr",
    "jun",
    "jul",
    "aug",
    "sep",
    "sept",
    "oct",
    "nov",
    "dec",
];

/// The parts of a title that qualify it: bracketed text, and whatever follows a separator
/// ("Artist - Song - Manchester 2005" → "Manchester 2005"), as words.
fn qualifier_segments(title: &str) -> Vec<Vec<String>> {
    let (outside, brackets) = split_brackets(title);
    let mut segments: Vec<Vec<String>> = brackets.iter().map(|b| tokens(b)).collect();
    let mut pieces = vec![outside.as_str()];
    for separator in [" - ", " \u{2013} ", " \u{2014} ", " | ", ": "] {
        pieces = pieces.iter().flat_map(|p| p.split(separator)).collect();
    }
    segments.extend(pieces.iter().skip(1).map(|p| tokens(p)));
    segments.retain(|s| !s.is_empty());
    segments
}

/// Words that say a qualifier is the same recording, not another one.
fn is_same_recording_word(word: &str) -> bool {
    matches!(
        word,
        "remaster"
            | "remastered"
            | "remastering"
            | "reissue"
            | "edition"
            | "anniversary"
            | "deluxe"
            | "album"
            | "lp"
            | "ep"
            | "original"
            | "official"
            | "stereo"
            | "mono"
            | "explicit"
            | "clean"
    )
}

/// Concert videos are often titled by place and date and never say "live":
/// "Song (Manchester, England - June 18, 2019)", "Song [Seattle '89]", "Song - Manchester 2005",
/// "Song (Manchester 11-7-2025)".
fn looks_like_a_date(
    title: &str,
    words: &[String],
    segments: &[Vec<String>],
    is_known: &dyn Fn(&str) -> bool,
) -> bool {
    let is_day_or_year = |w: &String| {
        w.bytes().all(|b| b.is_ascii_digit()) && (w.len() <= 2 && w != "0" || is_year(w))
    };
    let month_and_number = words.iter().enumerate().any(|(i, w)| {
        MONTHS.contains(&w.as_str())
            && (words.get(i + 1).is_some_and(is_day_or_year)
                || i > 0 && words.get(i - 1).is_some_and(is_day_or_year))
    });
    if month_and_number || has_numeric_date(title) {
        return true;
    }
    // '89
    let chars: Vec<char> = title.chars().collect();
    let short_year = chars.windows(3).enumerate().any(|(i, w)| {
        matches!(w[0], '\'' | '\u{2019}')
            && w[1].is_ascii_digit()
            && w[2].is_ascii_digit()
            && chars.get(i + 3).is_none_or(|c| !c.is_alphanumeric())
            && (i == 0 || !chars[i - 1].is_alphanumeric())
    });
    if short_year {
        return true;
    }
    // A place and a year: "(Night of the Proms, Belgium, 2023)", "- Manchester 2005". Not
    // "(Remastered 2011)" or "(2015 Mix)".
    segments.iter().any(|ws| {
        ws.iter().any(|w| is_year(w))
            && !ws
                .iter()
                .any(|w| is_same_recording_word(w) || is_take_word(w))
            && ws
                .iter()
                .any(|w| !is_year(w) && !is_known(w) && !w.bytes().all(|b| b.is_ascii_digit()))
    })
}

/// "11-7-2025", "2019.06.18", "18/06/19".
fn has_numeric_date(title: &str) -> bool {
    title
        .split(|c: char| !(c.is_ascii_digit() || matches!(c, '-' | '.' | '/')))
        .any(|run| {
            let Some(separator) = run.chars().find(|c| !c.is_ascii_digit()) else {
                return false;
            };
            let parts: Vec<&str> = run.split(separator).collect();
            if parts.len() != 3
                || parts
                    .iter()
                    .any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()))
            {
                return false;
            }
            let lengths: Vec<usize> = parts.iter().map(|p| p.len()).collect();
            matches!(
                lengths.as_slice(),
                [1..=2, 1..=2, 2 | 4] | [4, 1..=2, 1..=2]
            )
        })
}

fn is_take_word(word: &str) -> bool {
    matches!(
        word,
        "version"
            | "edit"
            | "mix"
            | "take"
            | "cut"
            | "rework"
            | "reimagined"
            | "rerecorded"
            | "redux"
    )
}

/// A qualifier naming another take of the song: "(Beat Version)", "(Radio Edit)",
/// "[Extended Mix]", "(Single Version)". Not "(Remastered Version)" or "(Album Version)".
fn alternate_take(segments: &[Vec<String>], is_known: &dyn Fn(&str) -> bool) -> bool {
    segments.iter().any(|ws| {
        ws.iter().any(|w| is_take_word(w))
            && ws.iter().any(|w| {
                !is_take_word(w)
                    && !is_same_recording_word(w)
                    && !is_year(w)
                    && (!is_known(w) || matches!(w.as_str(), "single" | "radio" | "extended"))
            })
    })
}

/// The artist most of the results agree on, for a tab that doesn't name one: the "Artist" of
/// "Artist - Title" titles and the names of official artist channels, among results that have
/// the song's title. `None` unless at least three results, and two in five of those with the
/// title, agree.
fn infer_artist(song: &Song, candidates: &[VideoCandidate]) -> Option<String> {
    let phrase: Vec<&str> = song.title_phrase.iter().map(String::as_str).collect();
    if phrase.is_empty() {
        return None;
    }
    let mut votes: Vec<(String, String, usize)> = Vec::new();
    let mut with_title = 0;
    for c in candidates {
        if !contains_phrase(&tokens(&c.title), &phrase) {
            continue;
        }
        with_title += 1;
        let mut names: Vec<&str> = Vec::new();
        for separator in [" - ", " \u{2013} ", " \u{2014} ", ": "] {
            if let Some((before, after)) = c.title.split_once(separator) {
                if contains_phrase(&tokens(after), &phrase)
                    && !contains_phrase(&tokens(before), &phrase)
                {
                    names.push(before.trim());
                }
                break;
            }
        }
        let channel = c.channel.trim_end();
        if let Some(topic) = channel.strip_suffix("- Topic") {
            names.push(topic.trim_end());
        } else if c.badge == ChannelBadge::OfficialArtist {
            names.push(channel);
        }
        let mut counted: Vec<String> = Vec::new();
        for name in names {
            let key = compact(name);
            if key.is_empty() || counted.contains(&key) {
                continue;
            }
            match votes.iter_mut().find(|(k, _, _)| *k == key) {
                Some(vote) => vote.2 += 1,
                None => votes.push((key.clone(), name.to_string(), 1)),
            }
            counted.push(key);
        }
    }
    let (_, name, count) = votes.into_iter().max_by_key(|(_, _, n)| *n)?;
    (count >= 3 && count * 5 >= with_title * 2).then_some(name)
}

/// "Filmed at …", "Recorded live in …", "Watch Metallica perform …".
fn live_description(words: &[String]) -> bool {
    const PHRASES: &[&[&str]] = &[
        &["filmed", "at"],
        &["filmed", "live"],
        &["filmed", "in"],
        &["recorded", "live"],
        &["live", "at"],
        &["live", "in"],
        &["live", "from"],
        &["in", "concert"],
        &["perform"],
        &["performs"],
        &["performing"],
    ];
    PHRASES.iter().any(|p| contains_phrase(words, p))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Version {
    Commentary,
    Album,
    Lesson,
    Cover,
    Altered,
    BackingTrack,
    Live,
    Acoustic,
    Instrumental,
    Demo,
    /// Another take: "(Single Version)", "(Radio Edit)". Detected from qualifiers, not phrases.
    Alternate,
}

struct VersionInfo {
    version: Version,
    /// Every word in [`VersionInfo::phrases`], so they don't count as unexplained.
    words: &'static [&'static str],
    detect: fn(&[String]) -> bool,
    penalty: f64,
    /// What a video of this version is attached as; `None` keeps "Original".
    label: Option<&'static str>,
    reason: &'static str,
    /// Added to the search when the tab asks for this version.
    query_word: Option<&'static str>,
}

macro_rules! phrases {
    ($($phrase:expr),* $(,)?) => {
        |words: &[String]| {
            [$($phrase),*].iter().any(|p: &&str| {
                let p: Vec<&str> = p.split(' ').collect();
                contains_phrase(words, &p)
            })
        }
    };
}

/// In label priority order: the first detected version with a label names the video.
const VERSIONS: &[VersionInfo] = &[
    VersionInfo {
        version: Version::Live,
        words: &[
            "live",
            "concert",
            "unplugged",
            "tour",
            "festival",
            "session",
            "sessions",
            "soundcheck",
            "sound",
            "check",
            "rehearsal",
            "bootleg",
            "tiny",
            "desk",
            "lounge",
            "stage",
            "kexp",
        ],
        detect: phrases![
            "live",
            "concert",
            "tour",
            "festival",
            "session",
            "sessions",
            "soundcheck",
            "sound check",
            "rehearsal",
            "bootleg",
            "tiny desk",
            "live lounge",
            "on stage",
            "kexp"
        ],
        penalty: 0.25,
        label: Some("Live"),
        reason: "Live",
        query_word: Some("live"),
    },
    VersionInfo {
        version: Version::Acoustic,
        words: &["acoustic", "unplugged", "stripped"],
        detect: phrases!["acoustic", "unplugged", "stripped"],
        penalty: 0.25,
        label: Some("Acoustic"),
        reason: "Acoustic",
        query_word: Some("acoustic"),
    },
    VersionInfo {
        version: Version::Cover,
        words: &["cover", "covers", "covered", "tribute", "rendition"],
        detect: phrases!["cover", "covers", "covered by", "tribute", "rendition"],
        penalty: 0.45,
        label: Some("Cover"),
        reason: "Cover",
        query_word: None,
    },
    VersionInfo {
        version: Version::Lesson,
        words: &[
            "lesson",
            "lessons",
            "tutorial",
            "how",
            "play",
            "tab",
            "tabs",
            "chords",
            "playthrough",
            "through",
            "breakdown",
            "riff",
            "riffs",
            "learn",
            "songsterr",
            "pro",
            "cam",
            "pov",
            "solo",
            "guitar",
            "bass",
            "drum",
            "drums",
        ],
        detect: phrases![
            "lesson",
            "lessons",
            "tutorial",
            "how to play",
            "tab",
            "tabs",
            "chords",
            "playthrough",
            "play through",
            "breakdown",
            "learn",
            "songsterr",
            "guitar pro",
            "guitar cam",
            "drum cam",
            "pov"
        ],
        penalty: 0.5,
        label: Some("Lesson"),
        reason: "Lesson or playthrough",
        query_word: None,
    },
    VersionInfo {
        version: Version::BackingTrack,
        words: &[
            "backing",
            "track",
            "backingtrack",
            "jam",
            "karaoke",
            "drumless",
            "bassless",
            "guitarless",
            "no",
            "vocals",
            "minus",
            "one",
            "along",
            "playalong",
        ],
        detect: phrases![
            "backing track",
            "backingtrack",
            "jam track",
            "karaoke",
            "drumless",
            "bassless",
            "guitarless",
            "no guitar",
            "no vocals",
            "minus one",
            "play along",
            "playalong"
        ],
        penalty: 0.35,
        label: Some("Backing Track"),
        reason: "Backing track",
        query_word: None,
    },
    VersionInfo {
        version: Version::Instrumental,
        words: &[
            "instrumental",
            "isolated",
            "only",
            "acapella",
            "acappella",
            "capella",
            "cappella",
            "stems",
        ],
        detect: phrases![
            "instrumental",
            "isolated",
            "guitar only",
            "vocals only",
            "only guitar",
            "acapella",
            "acappella",
            "a cappella",
            "a capella",
            "capella",
            "cappella",
            "stems"
        ],
        penalty: 0.15,
        label: Some("Instrumental"),
        reason: "Instrumental",
        query_word: Some("instrumental"),
    },
    VersionInfo {
        version: Version::Demo,
        words: &["demo"],
        detect: phrases!["demo"],
        penalty: 0.15,
        label: Some("Demo"),
        reason: "Demo",
        query_word: Some("demo"),
    },
    VersionInfo {
        version: Version::Altered,
        words: &[
            "slowed",
            "sped",
            "speed",
            "up",
            "nightcore",
            "daycore",
            "reverb",
            "8d",
            "boosted",
            "remix",
            "remixed",
            "mashup",
            "lofi",
            "lo",
            "fi",
            "reversed",
            "8bit",
            "bit",
            "piano",
            "orchestral",
            "symphonic",
            "medley",
            "hour",
            "hours",
            "loop",
            "chipmunk",
            "but",
        ],
        detect: phrases![
            "slowed",
            "sped up",
            "speed up",
            "nightcore",
            "daycore",
            "reverb",
            "8d",
            "bass boosted",
            "remix",
            "remixed",
            "mashup",
            "lofi",
            "lo fi",
            "reversed",
            "8 bit",
            "8bit",
            "piano version",
            "orchestral",
            "symphonic",
            "medley",
            "1 hour",
            "10 hours",
            "hour loop",
            "chipmunk",
            "but"
        ],
        penalty: 0.45,
        label: None,
        reason: "Remix or edit",
        query_word: None,
    },
    VersionInfo {
        version: Version::Alternate,
        words: &[
            "version",
            "edit",
            "mix",
            "take",
            "cut",
            "rework",
            "reimagined",
            "rerecorded",
            "redux",
            "single",
            "radio",
            "extended",
        ],
        detect: |_| false,
        penalty: 0.15,
        label: None,
        reason: "Alternate version",
        query_word: None,
    },
    VersionInfo {
        version: Version::Commentary,
        words: &[
            "reaction",
            "reacts",
            "react",
            "reacting",
            "review",
            "analysis",
            "explained",
            "interview",
            "documentary",
            "behind",
            "scenes",
            "making",
            "podcast",
            "roasts",
            "first",
            "time",
            "hearing",
            "coach",
            "shorts",
            "ever",
            "heard",
        ],
        detect: phrases![
            "reaction",
            "reacts",
            "react",
            "reacting",
            "review",
            "analysis",
            "explained",
            "interview",
            "documentary",
            "behind the scenes",
            "making of",
            "podcast",
            "roasts",
            "first time hearing",
            "vocal coach",
            "shorts",
            "ever heard"
        ],
        penalty: 0.6,
        label: None,
        reason: "Reaction or review",
        query_word: None,
    },
    VersionInfo {
        version: Version::Album,
        words: &["album", "ep", "stream", "whole", "discography"],
        detect: phrases![
            "full album",
            "full ep",
            "album stream",
            "whole album",
            "discography"
        ],
        penalty: 0.6,
        label: None,
        reason: "Full album",
        query_word: None,
    },
];

fn info(version: Version) -> &'static VersionInfo {
    VERSIONS
        .iter()
        .find(|v| v.version == version)
        .expect("every version has an entry")
}
