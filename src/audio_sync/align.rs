//! Aligning a capture to the tab's reference features.
//!
//! Two passes:
//! 1. **Coarse** (chroma, 10 Hz): a local alignment by dynamic programming finds the stretch of
//!    the tab that the capture contains and a warping path through it. Steps are limited to
//!    slopes between ½ and 2, either side may start and end anywhere, so intros, outros and
//!    partial captures need no special handling.
//! 2. **Fine** (onsets, 100 Hz): around every bar start, the tab's onset pattern is slid against
//!    the capture's near where the coarse path says it is. The longest run of bars that agree
//!    with each other fixes the recording's tempo; bars elsewhere are kept only if they agree
//!    *and* fit that tempo. The rest (thin stretches where the coarse path can wander: held
//!    chords, quiet intros) keep time between the trusted bars and are nudged onto their
//!    onsets where those can be heard close by: a groove that repeats fits just as well a
//!    sixteenth or an eighth note away, and a bar pulled that far has found the wrong beat.

use super::features::{CHROMA_RATE, Features, ONSET_RATE};

/// Chroma similarity a frame pair has to beat to count in the path's favour.
const MATCH_THRESHOLD: f32 = 0.2;
/// What a frame pair costs when either side is silent: no evidence, but not free.
const SILENCE_GAIN: f32 = -0.02;
/// A path starting `i` frames into the tab needs at least this fraction of `i` frames of video
/// before it (the recording may run faster than the tab), less a little slack (frames).
const START_RUSH: f32 = 0.85;
const START_SLACK: f32 = 1.0 * CHROMA_RATE as f32;
/// What each audible tab frame costs when a path starts after it although the capture was
/// already running: a riff that repeats fits in several places, and the right one is where
/// the tab's start meets the music's.
const START_SKIP_COST: f32 = 0.15;
/// Cost of a tempo-changing step: where the evidence is thin (a held chord, a rest) the path
/// should keep the tab's tempo rather than wander.
const SKEW_PENALTY: f32 = 0.3;
/// What an onset in the tab landing on an onset in the capture adds to a frame pair's gain.
/// Chroma says which part of the song this is; onsets say exactly when.
const ONSET_GAIN: f32 = 0.4;
/// The DP table is one byte per frame pair; refuse to go past this (about 18 × 18 minutes).
const MAX_CELLS: usize = 120_000_000;
/// Shortest path worth trusting (chroma frames).
const MIN_PATH: usize = (12.0 * CHROMA_RATE) as usize;
/// A path whose matched frames are less alike than this isn't an alignment.
const MIN_SIMILARITY: f64 = 0.25;
/// A tab with this much of its pitched content in the bass register is a bass tab.
const BASS_TAB_SHARE: f32 = 0.6;
/// How many times better than the tab as written a transposed alignment must score.
const TRANSPOSED_MARGIN: f32 = 1.15;
/// How many times better the alignment must score than the same tab transposed a tritone.
const MIN_CONTROL_RATIO: f32 = 3.0;
/// Path points either side that the lag is averaged over.
const LAG_SMOOTHING: usize = (2.0 * CHROMA_RATE) as usize;

/// The onset pattern compared around each bar start (seconds before, after)…
const FINE_WINDOW: (f64, f64) = (1.0, 1.5);
/// …widened, where the tab is sparse, until it holds this many onsets or is this many times
/// as long.
const FINE_MIN_ONSETS: usize = 6;
const FINE_MAX_WIDENING: f64 = 4.0;
/// Fewest onsets a pattern needs to be placed at all.
const FINE_FEWEST_ONSETS: usize = 3;
/// How far the fine pass may move a bar from the coarse path (onset frames).
const FINE_MAX_LAG: i32 = 40;
/// Weakest onset correlation that counts as a match, and how far it must stand above the
/// typical correlation across the lags tried (noise correlates with anything a little).
const FINE_MIN_CORRELATION: f64 = 0.3;
const FINE_MIN_PROMINENCE: f64 = 0.15;
/// Fewest agreeing bars in a row to start tracking from.
const TRACK_MIN_SEED: usize = 4;
/// Bars the tracker's tempo is measured over, and how far that tempo may stray from the
/// seed's overall tempo.
const TRACK_TEMPO_BARS: usize = 6;
const TRACK_TEMPO_LEEWAY: f64 = 0.06;
/// A bar's own estimate is kept when it's this close (seconds) to what its neighbours imply.
const FINE_AGREEMENT: f64 = 0.05;
/// How far a bar that isn't trusted may be nudged from where the bars around it put it, as a
/// fraction of its length: half a sixteenth note of a bar of 4/4. A groove that repeats fits
/// just as well a sixteenth or an eighth away, and a bar pulled that far found the wrong beat.
const PLACE_MAX_NUDGE: f64 = 1.0 / 32.0;
/// Estimates looked at on each side of a bar when checking its own.
const FINE_NEIGHBOURS: usize = 2;

#[derive(Debug, Clone, PartialEq)]
pub struct Alignment {
    /// Capture seconds at which each bar starts (one per entry of `bar_starts`). Bars outside
    /// `first_bar..=last_bar` continue at the tempo the aligned bars were played at.
    pub bar_times: Vec<f64>,
    /// Per bar: its own onsets were found in the capture, and its time comes from them.
    pub heard: Vec<bool>,
    /// The bars the capture was actually matched against.
    pub first_bar: usize,
    pub last_bar: usize,
    /// Bars in that range whose onsets were found in the capture.
    pub matched_bars: usize,
    /// Mean chroma similarity along the path (−1…1; unrelated audio scores near 0).
    pub similarity: f64,
    /// Semitones the capture sounds above the tab (−5…6).
    pub transpose: i8,
    /// Capture seconds per score second over the aligned range.
    pub stretch: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlignError {
    /// Too much audio and tab for the table.
    TooLong,
    /// No convincing alignment.
    NoMatch,
}

/// Align `audio` to `reference`; `bar_starts` are the played bars' starts in score seconds.
/// `video_start`: the capture second at which the video itself starts (negative when listening
/// began partway through), if known. The tab can't start before the video does, which settles
/// which repeat of a riff a short capture is hearing.
pub fn align(
    reference: &Features,
    audio: &Features,
    bar_starts: &[f64],
    video_start: Option<f64>,
) -> Result<Alignment, AlignError> {
    // A tab that's mostly bass is listened for in the bass register: higher up, the rest of
    // the band covers it.
    let total = |frames: &[[f32; 12]]| frames.iter().flatten().sum::<f32>();
    let low = total(&reference.bass) > BASS_TAB_SHARE * total(&reference.chroma)
        && audio.bass.len() == audio.chroma.len();
    let ref_chroma = Chroma::new(&reference.chroma, false);
    let audio_chroma = Chroma::new(if low { &audio.bass } else { &audio.chroma }, true);
    let (n, m) = (ref_chroma.frames.len(), audio_chroma.frames.len());
    if n < MIN_PATH || m < MIN_PATH {
        return Err(AlignError::NoMatch);
    }
    if n.saturating_mul(m) > MAX_CELLS {
        return Err(AlignError::TooLong);
    }

    let ref_onset = prepare_onset(&reference.onset, true);
    let audio_onset = prepare_onset(&audio.onset, false);
    let beats = (coarse_onsets(&ref_onset, n), coarse_onsets(&audio_onset, m));
    let beats = Some((&beats.0[..], &beats.1[..]));
    let earliest = video_start.map(|secs| (secs * CHROMA_RATE) as f32);

    // As written, and in every other key: tabs are often in standard tuning for a recording
    // that's a semitone down, or written for a capo or a transposing instrument. Another key
    // has to line up clearly better than the written one to be believed.
    let mut best = local_align(&ref_chroma, &audio_chroma, beats, earliest).map(|p| (p, 0usize));
    let bar = best
        .as_ref()
        .map_or(0.0, |(p, _)| TRANSPOSED_MARGIN * p.score);
    for shift in 1..12 {
        if let Some(shifted) =
            local_align(&ref_chroma.rotated(shift), &audio_chroma, beats, earliest)
            && shifted.score > bar
            && best
                .as_ref()
                .is_none_or(|(p, at)| *at == 0 || shifted.score > p.score)
        {
            best = Some((shifted, shift));
        }
    }
    let (path, shift) = best.ok_or(AlignError::NoMatch)?;
    // Is it the tab at all? Any two pieces of music line up somewhat; aligning the tab a
    // tritone away (as wrong as a key gets) shows how much "somewhat" is for this pair.
    // (On chroma alone: onsets line up just as well in the wrong key.)
    let control = local_align(
        &ref_chroma.rotated((shift + 6) % 12),
        &audio_chroma,
        None,
        earliest,
    );
    if control.is_some_and(|c| path.chroma_score < MIN_CONTROL_RATIO * c.chroma_score) {
        return Err(AlignError::NoMatch);
    }
    if path.cells.len() < MIN_PATH || path.similarity < MIN_SIMILARITY {
        return Err(AlignError::NoMatch);
    }

    let coarse = Coarse::new(&path.cells);
    let (s_first, s_last) = (coarse.score[0], coarse.score[coarse.score.len() - 1]);
    let in_range: Vec<usize> = (0..bar_starts.len())
        .filter(|&k| (s_first..=s_last).contains(&bar_starts[k]))
        .collect();
    let (Some(&first_bar), Some(&last_bar)) = (in_range.first(), in_range.last()) else {
        return Err(AlignError::NoMatch);
    };

    let ref_onsets = onset_counts(&reference.onset);
    let finder = Finder {
        reference: &ref_onset,
        onsets: &ref_onsets,
        audio: &audio_onset,
        limits: (s_first, s_last),
    };
    let starts: Vec<f64> = in_range.iter().map(|&k| bar_starts[k]).collect();

    // First guesses, from the coarse path: each bar's offset from the tab's own clock (capture
    // seconds minus score seconds) where its onsets were found near the path. In a real
    // recording that offset drifts smoothly from bar to bar, so a bar far from what its
    // neighbours imply locked onto the wrong onsets.
    let offsets: Vec<Option<(f64, f64)>> = starts
        .iter()
        .map(|&s| {
            finder
                .lag(s, |at| coarse.audio_at(at))
                .map(|lag| (s, coarse.audio_at(s) + lag / ONSET_RATE - s))
        })
        .collect();
    let agrees: Vec<bool> = (0..starts.len())
        .map(
            |slot| match (offsets[slot], expected_offset(&offsets, slot, starts[slot])) {
                (Some((_, own)), Some(expected)) => (own - expected).abs() <= FINE_AGREEMENT,
                _ => false,
            },
        )
        .collect();

    // The longest run of bars that agree with each other is where the capture is surely the
    // tab, and its tempo is the recording's. Bars elsewhere are trusted too if they agree with
    // their neighbours *and* sit where that tempo (give or take) would put them: a thin
    // stretch (a held chord, a quiet intro) can agree with itself about nonsense.
    let mut times: Vec<f64> = starts
        .iter()
        .zip(&offsets)
        .map(|(&s, offset)| s + offset.map_or(coarse.audio_at(s) - s, |(_, o)| o))
        .collect();
    // Per slot: the bar's own onsets were found, and its time comes from them.
    let mut heard: Vec<bool> = offsets.iter().map(Option::is_some).collect();
    if let Some(seed) = longest_run(&agrees).filter(|run| run.len() >= TRACK_MIN_SEED) {
        let overall =
            (times[seed.end - 1] - times[seed.start]) / (starts[seed.end - 1] - starts[seed.start]);
        let plausible = |from: usize, to: usize, times: &[f64]| {
            let rate = (times[to] - times[from]) / (starts[to] - starts[from]);
            (rate / overall - 1.0).abs() <= TRACK_TEMPO_LEEWAY
        };
        let mut trusted = vec![false; starts.len()];
        trusted[seed.clone()].fill(true);
        let mut last = seed.end - 1;
        for slot in seed.end..starts.len() {
            if agrees[slot] && plausible(last, slot, &times) {
                trusted[slot] = true;
                last = slot;
            }
        }
        let mut last = seed.start;
        for slot in (0..seed.start).rev() {
            if agrees[slot] && plausible(slot, last, &times) {
                trusted[slot] = true;
                last = slot;
            }
        }
        heard.clone_from(&trusted);

        // Everything else keeps time between or beyond the trusted bars, and is nudged onto
        // its onsets where they can be heard near there. Only near: onsets found further off
        // are the groove's next beat along, not this bar (in 77 real recordings one bar in ten
        // was pulled a sixteenth or an eighth note out this way). A tempo that really drifts
        // moves bar after bar a little, and each bar found carries the next.
        let anchors: Vec<usize> = (0..starts.len()).filter(|&slot| trusted[slot]).collect();
        let mut place = |slot: usize, predicted: f64, rate: f64, times: &mut Vec<f64>| -> bool {
            let s = starts[slot];
            let length = match starts.get(slot + 1) {
                Some(next) => next - s,
                None => s - starts[slot - 1],
            };
            let reach = (PLACE_MAX_NUDGE * rate * length).max(FINE_AGREEMENT);
            let found = finder
                .lag(s, |at| predicted + rate * (at - s))
                .filter(|lag| (lag / ONSET_RATE).abs() <= reach);
            heard[slot] = found.is_some();
            times[slot] = predicted + found.unwrap_or(0.0) / ONSET_RATE;
            found.is_some()
        };
        for pair in anchors.windows(2) {
            let (left, right) = (pair[0], pair[1]);
            // Between two trusted bars: on the line from the last bar found to the next
            // trusted one.
            let mut from = left;
            for slot in left + 1..right {
                let rate = (times[right] - times[from]) / (starts[right] - starts[from]);
                let predicted = times[from] + rate * (starts[slot] - starts[from]);
                if place(slot, predicted, rate, &mut times) {
                    from = slot;
                }
            }
        }
        // Beyond the outermost trusted bars: step outwards at the tempo of the last few bars.
        let (first, last) = (anchors[0], anchors[anchors.len() - 1]);
        let leeway = overall * (1.0 - TRACK_TEMPO_LEEWAY)..=overall * (1.0 + TRACK_TEMPO_LEEWAY);
        for slot in last + 1..starts.len() {
            let (fixed, back) = (
                slot - 1,
                (slot - 1).saturating_sub(TRACK_TEMPO_BARS).max(first),
            );
            let rate = if back == fixed {
                overall
            } else {
                ((times[fixed] - times[back]) / (starts[fixed] - starts[back]))
                    .clamp(*leeway.start(), *leeway.end())
            };
            let predicted = times[fixed] + rate * (starts[slot] - starts[fixed]);
            place(slot, predicted, rate, &mut times);
        }
        for slot in (0..first).rev() {
            let (fixed, back) = (slot + 1, (slot + 1 + TRACK_TEMPO_BARS).min(last));
            let rate = if back == fixed {
                overall
            } else {
                ((times[back] - times[fixed]) / (starts[back] - starts[fixed]))
                    .clamp(*leeway.start(), *leeway.end())
            };
            let predicted = times[fixed] + rate * (starts[slot] - starts[fixed]);
            place(slot, predicted, rate, &mut times);
        }
    } else {
        // No stretch to trust outright: settle each bar against its neighbours instead.
        for slot in 0..starts.len() {
            let own = offsets[slot].map(|(_, o)| o);
            let (offset, own) = match (own, expected_offset(&offsets, slot, starts[slot])) {
                (Some(own), Some(expected)) if (own - expected).abs() <= FINE_AGREEMENT => {
                    (own, true)
                }
                (Some(own), None) => (own, true),
                (_, Some(expected)) => (expected, false),
                (None, None) => (coarse.audio_at(starts[slot]) - starts[slot], false),
            };
            heard[slot] = own;
            times[slot] = starts[slot] + offset;
        }
    }
    let mut bar_times = vec![0.0; bar_starts.len()];
    let mut bar_heard = vec![false; bar_starts.len()];
    for ((&k, &t), &h) in in_range.iter().zip(&times).zip(&heard) {
        bar_times[k] = t;
        bar_heard[k] = h;
    }
    let matched_bars = heard.iter().filter(|h| **h).count();
    // Outside the aligned range, carry on at the tempo the aligned bars were played at.
    let span = bar_starts[last_bar] - bar_starts[first_bar];
    let stretch = if span >= 20.0 {
        ((bar_times[last_bar] - bar_times[first_bar]) / span).clamp(0.5, 2.0)
    } else {
        1.0
    };
    // Except through rests the tab opens with: they were written to fill the recording's
    // intro, and there is nothing in them to hear a tempo from.
    let first_note = reference
        .onset
        .iter()
        .position(|v| *v > 0.0)
        .map_or(0.0, |frame| (frame + 2) as f64 / ONSET_RATE);
    for k in (0..first_bar).rev() {
        let rate = if bar_starts[k + 1] <= first_note {
            1.0
        } else {
            stretch
        };
        bar_times[k] = bar_times[k + 1] - rate * (bar_starts[k + 1] - bar_starts[k]);
    }
    for k in last_bar + 1..bar_starts.len() {
        bar_times[k] = bar_times[k - 1] + stretch * (bar_starts[k] - bar_starts[k - 1]);
    }
    // Bars can't cross: keep at least a quarter of each bar's length.
    for k in 1..bar_times.len() {
        let floor = bar_times[k - 1] + 0.25 * (bar_starts[k] - bar_starts[k - 1]);
        bar_times[k] = bar_times[k].max(floor);
    }

    let transpose = if shift > 6 {
        shift as i8 - 12
    } else {
        shift as i8
    };
    Ok(Alignment {
        bar_times,
        heard: bar_heard,
        first_bar,
        last_bar,
        matched_bars,
        similarity: path.similarity,
        transpose,
        stretch,
    })
}

/// Chroma frames made comparable: compressed, smoothed, centred and unit length, so the dot
/// product of two frames is their correlation. Silent frames are all zero.
struct Chroma {
    frames: Vec<[f32; 12]>,
    silent: Vec<bool>,
}

impl Chroma {
    /// `power`: the raw values are energies (measured audio) rather than amplitudes.
    fn new(raw: &[[f32; 12]], power: bool) -> Self {
        let compressed: Vec<[f32; 12]> = raw
            .iter()
            .map(|f| f.map(|v| if power { v.max(0.0).sqrt() } else { v.max(0.0) }))
            .collect();
        let at = |i: usize| compressed[i.min(compressed.len().saturating_sub(1))];
        let smoothed: Vec<[f32; 12]> = (0..compressed.len())
            .map(|i| {
                let (a, b, c) = (at(i.saturating_sub(1)), at(i), at(i + 1));
                std::array::from_fn(|pc| 0.25 * a[pc] + 0.5 * b[pc] + 0.25 * c[pc])
            })
            .collect();

        let mut levels: Vec<f32> = smoothed.iter().map(|f| f.iter().sum()).collect();
        let energy = levels.clone();
        levels.sort_by(f32::total_cmp);
        let loud = levels.get(levels.len() * 9 / 10).copied().unwrap_or(0.0);
        let floor = (0.02 * loud).max(f32::MIN_POSITIVE);

        let mut silent = Vec::with_capacity(smoothed.len());
        let frames = smoothed
            .into_iter()
            .zip(energy)
            .map(|(f, e)| {
                let mean = e / 12.0;
                let centred = f.map(|v| v - mean);
                let norm = centred.iter().map(|v| v * v).sum::<f32>().sqrt();
                let quiet = e < floor || norm <= f32::MIN_POSITIVE;
                silent.push(quiet);
                if quiet {
                    [0.0; 12]
                } else {
                    centred.map(|v| v / norm)
                }
            })
            .collect();
        Self { frames, silent }
    }

    /// Every frame moved up `semitones` pitch classes.
    fn rotated(&self, semitones: usize) -> Self {
        Self {
            frames: self
                .frames
                .iter()
                .map(|f| std::array::from_fn(|pc| f[(pc + 12 - semitones) % 12]))
                .collect(),
            silent: self.silent.clone(),
        }
    }
}

struct Path {
    /// (reference frame, audio frame), increasing in both.
    cells: Vec<(u32, u32)>,
    score: f32,
    /// The part of `score` that chroma earned (no onset gains, no step penalties).
    chroma_score: f32,
    similarity: f64,
}

/// Best local alignment of two chroma sequences (Smith–Waterman with DTW steps). `beats`:
/// onset strength per chroma frame (0…1) of each side, to reward onsets that coincide.
///
/// `earliest`: the audio frame at which the video starts. A path may only start where the tab
/// before it still fits after that (played up to [`START_RUSH`] faster than written).
fn local_align(
    reference: &Chroma,
    audio: &Chroma,
    beats: Option<(&[f32], &[f32])>,
    earliest: Option<f32>,
) -> Option<Path> {
    let (n, m) = (reference.frames.len(), audio.frames.len());
    // How each cell was reached: 0 = path starts here, 1 = (−1, −1), 2 = (−1, −2), 3 = (−2, −1).
    let mut back = vec![0u8; n * m];
    let mut two_up = vec![0.0f32; m];
    let mut one_up = vec![0.0f32; m];
    let mut row = vec![0.0f32; m];
    let mut best = (0.0f32, 0usize, 0usize);
    // audible[i]: tab frames before `i` with something to hear in them.
    let mut audible = Vec::with_capacity(n + 1);
    audible.push(0u32);
    for silent in &reference.silent {
        audible.push(audible[audible.len() - 1] + u32::from(!silent));
    }

    for i in 0..n {
        let r = &reference.frames[i];
        let r_silent = reference.silent[i];
        let earliest_start = earliest.map(|e| e + START_RUSH * i as f32 - START_SLACK);
        let r_beat = beats.map_or(0.0, |(b, _)| ONSET_GAIN * b[i]);
        let back_row = &mut back[i * m..(i + 1) * m];
        for j in 0..m {
            let mut gain = if r_silent || audio.silent[j] {
                SILENCE_GAIN
            } else {
                dot(r, &audio.frames[j]) - MATCH_THRESHOLD
            };
            if let Some((_, a_beats)) = beats {
                gain += r_beat * a_beats[j];
            }
            // Starting here: impossible if the tab so far couldn't have fitted into the
            // video, and otherwise it costs whatever of the tab should have been heard in the
            // capture before this point but is being skipped.
            let mut from = match earliest_start {
                Some(earliest) if (j as f32) < earliest => f32::NEG_INFINITY,
                _ => {
                    let heard_from = earliest.map_or(0, |e| e.max(0.0) as usize);
                    let overlap = i.min(j.saturating_sub(heard_from));
                    -START_SKIP_COST * (audible[i] - audible[i - overlap]) as f32
                }
            };
            let mut step = 0u8;
            if i >= 1 && j >= 1 {
                if one_up[j - 1] > from {
                    from = one_up[j - 1];
                    step = 1;
                }
                if j >= 2 && one_up[j - 2] - SKEW_PENALTY > from {
                    from = one_up[j - 2] - SKEW_PENALTY;
                    step = 2;
                }
                if i >= 2 && two_up[j - 1] - SKEW_PENALTY > from {
                    from = two_up[j - 1] - SKEW_PENALTY;
                    step = 3;
                }
            }
            let total = gain + from;
            row[j] = total;
            back_row[j] = step;
            if total > best.0 {
                best = (total, i, j);
            }
        }
        std::mem::swap(&mut two_up, &mut one_up);
        std::mem::swap(&mut one_up, &mut row);
    }
    if best.0 <= 0.0 {
        return None;
    }

    let (score, mut i, mut j) = best;
    let mut cells = Vec::new();
    loop {
        cells.push((i as u32, j as u32));
        match back[i * m + j] {
            1 => (i, j) = (i - 1, j - 1),
            2 => (i, j) = (i - 1, j - 2),
            3 => (i, j) = (i - 2, j - 1),
            _ => break,
        }
    }
    cells.reverse();

    let (mut sum, mut count) = (0.0f64, 0usize);
    let mut chroma_score = 0.0f32;
    for &(i, j) in &cells {
        let (i, j) = (i as usize, j as usize);
        if !reference.silent[i] && !audio.silent[j] {
            let alike = dot(&reference.frames[i], &audio.frames[j]);
            chroma_score += alike - MATCH_THRESHOLD;
            sum += f64::from(alike);
            count += 1;
        } else {
            chroma_score += SILENCE_GAIN;
        }
    }
    Some(Path {
        cells,
        score,
        chroma_score,
        similarity: if count == 0 { 0.0 } else { sum / count as f64 },
    })
}

#[inline]
fn dot(a: &[f32; 12], b: &[f32; 12]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// The coarse path as a smooth function from score seconds to capture seconds.
struct Coarse {
    /// Score seconds of each path point, strictly increasing.
    score: Vec<f64>,
    /// Capture seconds minus score seconds there, smoothed.
    lag: Vec<f64>,
}

impl Coarse {
    fn new(cells: &[(u32, u32)]) -> Self {
        let score: Vec<f64> = cells
            .iter()
            .map(|&(i, _)| f64::from(i) / CHROMA_RATE)
            .collect();
        let raw: Vec<f64> = cells
            .iter()
            .map(|&(i, j)| (f64::from(j) - f64::from(i)) / CHROMA_RATE)
            .collect();
        // A centred mean doesn't bias a steady drift; near the ends the window shrinks to
        // stay centred.
        let lag = (0..raw.len())
            .map(|k| {
                let half = LAG_SMOOTHING.min(k).min(raw.len() - 1 - k);
                let window = &raw[k - half..=k + half];
                window.iter().sum::<f64>() / window.len() as f64
            })
            .collect();
        Self { score, lag }
    }

    /// Capture seconds for score seconds `s` (the tab's tempo beyond the path's ends).
    fn audio_at(&self, s: f64) -> f64 {
        let next = self.score.partition_point(|&t| t <= s);
        let lag = if next == 0 {
            self.lag[0]
        } else if next == self.score.len() {
            self.lag[next - 1]
        } else {
            let (s0, s1) = (self.score[next - 1], self.score[next]);
            let t = (s - s0) / (s1 - s0);
            self.lag[next - 1] + t * (self.lag[next] - self.lag[next - 1])
        };
        s + lag
    }
}

/// An onset envelope with its slow-moving level removed and unit RMS. `smooth` blurs the
/// reference's ideal impulses to about the width measured onsets have.
fn prepare_onset(raw: &[f32], smooth: bool) -> Vec<f32> {
    let mut env: Vec<f32> = raw.to_vec();
    if smooth {
        const KERNEL: [f32; 5] = [0.06, 0.24, 0.4, 0.24, 0.06];
        env = (0..raw.len())
            .map(|i| {
                KERNEL
                    .iter()
                    .enumerate()
                    .map(|(k, w)| {
                        (i + k)
                            .checked_sub(2)
                            .and_then(|x| raw.get(x))
                            .map_or(0.0, |v| v * w)
                    })
                    .sum()
            })
            .collect();
    }
    // Subtract the mean of the surrounding second (running sum), keep what stands out.
    let half = (0.5 * ONSET_RATE) as usize;
    let mut prefix = Vec::with_capacity(env.len() + 1);
    prefix.push(0.0f64);
    for v in &env {
        prefix.push(prefix[prefix.len() - 1] + f64::from(*v));
    }
    let mut out: Vec<f32> = (0..env.len())
        .map(|i| {
            let (lo, hi) = (i.saturating_sub(half), (i + half + 1).min(env.len()));
            let mean = (prefix[hi] - prefix[lo]) / (hi - lo) as f64;
            (f64::from(env[i]) - mean).max(0.0) as f32
        })
        .collect();
    let rms = (out.iter().map(|v| f64::from(v * v)).sum::<f64>() / out.len().max(1) as f64).sqrt();
    if rms > 0.0 {
        for v in &mut out {
            *v /= rms as f32;
        }
    }
    out
}

/// A prepared onset envelope at the chroma rate: the strongest onset within each of `frames`
/// chroma frames, scaled so a typical strong onset is 1.
fn coarse_onsets(env: &[f32], frames: usize) -> Vec<f32> {
    let per = (ONSET_RATE / CHROMA_RATE) as usize;
    let mut out: Vec<f32> = (0..frames)
        .map(|i| {
            // Chroma frame `i` is centred on onset frame `i * per`.
            let lo = (i * per).saturating_sub(per / 2).min(env.len());
            let hi = (i * per + per / 2).min(env.len());
            env[lo..hi].iter().copied().fold(0.0, f32::max)
        })
        .collect();
    let mut sorted: Vec<f32> = out.iter().copied().filter(|v| *v > 0.0).collect();
    sorted.sort_by(f32::total_cmp);
    let strong = sorted.get(sorted.len() * 9 / 10).copied().unwrap_or(0.0);
    if strong > 0.0 {
        for v in &mut out {
            *v = (*v / strong).min(1.0);
        }
    }
    out
}

/// Running count of onsets in the tab's raw onset envelope: `counts[f]` onsets start before
/// frame `f`.
fn onset_counts(raw: &[f32]) -> Vec<u32> {
    let mut counts = Vec::with_capacity(raw.len() + 1);
    let mut total = 0u32;
    counts.push(0);
    for (f, &v) in raw.iter().enumerate() {
        if v > 0.0 && (f == 0 || raw[f - 1] <= 0.0) {
            total += 1;
        }
        counts.push(total);
    }
    counts
}

fn sample(env: &[f32], position: f64) -> f64 {
    if position < 0.0 {
        return 0.0;
    }
    let i = position.floor() as usize;
    let frac = position - position.floor();
    let a = env.get(i).copied().map_or(0.0, f64::from);
    let b = env.get(i + 1).copied().map_or(0.0, f64::from);
    a + frac * (b - a)
}

/// The first maximal run of `true`s that's as long as any.
fn longest_run(flags: &[bool]) -> Option<std::ops::Range<usize>> {
    let mut best: Option<std::ops::Range<usize>> = None;
    let mut start = 0;
    for end in 0..=flags.len() {
        if end == flags.len() || !flags[end] {
            if end > start && best.as_ref().is_none_or(|b| end - start > b.len()) {
                best = Some(start..end);
            }
            start = end + 1;
        }
    }
    best
}

/// Finds the tab's onset pattern around a bar in the capture.
struct Finder<'a> {
    /// The tab's prepared onset envelope, and its running onset count.
    reference: &'a [f32],
    onsets: &'a [u32],
    /// The capture's prepared onset envelope.
    audio: &'a [f32],
    /// The score seconds the pattern may be taken from.
    limits: (f64, f64),
}

impl Finder<'_> {
    /// How far (onset frames, later = positive) the capture's onsets around the bar starting
    /// at score seconds `bar` sit from where `expected` (score seconds → capture seconds) puts
    /// them. `None` if the tab has too little to hear there or nothing convincing matches.
    fn lag(&self, bar: f64, expected: impl Fn(f64) -> f64) -> Option<f64> {
        let (reference, onsets, audio) = (self.reference, self.onsets, self.audio);
        let (s_first, s_last) = self.limits;
        // A pattern of one or two onsets fits anywhere: where the tab is sparse, look further.
        let mut widening = 1.0;
        let (from, to) = loop {
            let from = ((bar - widening * FINE_WINDOW.0).max(s_first) * ONSET_RATE).ceil() as usize;
            let to = (((bar + widening * FINE_WINDOW.1).min(s_last) * ONSET_RATE).floor() as usize)
                .min(reference.len().saturating_sub(1));
            if to <= from {
                return None;
            }
            let held = (onsets[to + 1] - onsets[from]) as usize;
            if held >= FINE_MIN_ONSETS {
                break (from, to);
            }
            if widening >= FINE_MAX_WIDENING {
                if held < FINE_FEWEST_ONSETS {
                    return None;
                }
                break (from, to);
            }
            widening = (widening * 1.5).min(FINE_MAX_WIDENING);
        };
        // Where each reference frame of the window is expected in the capture.
        let window: Vec<(f64, f64)> = (from..=to)
            .map(|f| {
                let at = expected(f as f64 / ONSET_RATE) * ONSET_RATE;
                (f64::from(reference[f]), at)
            })
            .collect();
        let ref_energy: f64 = window.iter().map(|(r, _)| r * r).sum();
        if ref_energy < 1.0 {
            return None;
        }

        // A long window only lines up if the tempo across it is right too, so sparse
        // stretches also try the pattern a little faster and slower about the bar itself.
        let pivot = expected(bar) * ONSET_RATE;
        let tempos: &[f64] = if widening > 1.0 {
            &[0.0, -0.01, 0.01, -0.02, 0.02, -0.03, 0.03]
        } else {
            &[0.0]
        };
        let mut best: Option<(f64, Vec<f64>, usize)> = None;
        for &tempo in tempos {
            let correlation = |lag: i32| -> f64 {
                let (mut cross, mut energy) = (0.0f64, 0.0f64);
                for &(r, at) in &window {
                    let at = pivot + (at - pivot) * (1.0 + tempo);
                    let a = sample(audio, at + f64::from(lag));
                    cross += r * a;
                    energy += a * a;
                }
                if energy <= 0.0 {
                    return 0.0;
                }
                // Prefer staying near the expected place when two lags fit about as well.
                let reach = f64::from(lag) / f64::from(FINE_MAX_LAG);
                cross / (ref_energy * energy).sqrt() * (1.0 - 0.25 * reach * reach)
            };
            let scores: Vec<f64> = (-FINE_MAX_LAG..=FINE_MAX_LAG).map(correlation).collect();
            let peak = (0..scores.len()).max_by(|&a, &b| scores[a].total_cmp(&scores[b]))?;
            if best.as_ref().is_none_or(|(top, _, _)| scores[peak] > *top) {
                best = Some((scores[peak], scores, peak));
            }
        }
        let (_, scores, peak) = best?;
        let mut sorted = scores.clone();
        sorted.sort_by(f64::total_cmp);
        let typical = sorted[sorted.len() / 2];
        if scores[peak] < FINE_MIN_CORRELATION || scores[peak] - typical < FINE_MIN_PROMINENCE {
            return None;
        }
        let mut lag = peak as f64 - f64::from(FINE_MAX_LAG);
        if peak > 0 && peak + 1 < scores.len() {
            let (before, here, after) = (scores[peak - 1], scores[peak], scores[peak + 1]);
            let curve = before - 2.0 * here + after;
            if curve < 0.0 {
                lag += 0.5 * (before - after) / curve;
            }
        }
        Some(lag)
    }
}

/// What the estimates around `slot` say its offset should be at score seconds `s`: the median
/// of straight lines between an estimate on each side (so one wild neighbour doesn't matter,
/// and a tempo that's changing bends the answer as little as possible). At the ends of the
/// range, where there's only one side, the line through that side's two nearest estimates.
fn expected_offset(offsets: &[Option<(f64, f64)>], slot: usize, s: f64) -> Option<f64> {
    let before: Vec<(f64, f64)> = offsets[..slot]
        .iter()
        .rev()
        .flatten()
        .take(FINE_NEIGHBOURS)
        .copied()
        .collect();
    let after: Vec<(f64, f64)> = offsets[slot + 1..]
        .iter()
        .flatten()
        .take(FINE_NEIGHBOURS)
        .copied()
        .collect();
    let through = |a: (f64, f64), b: (f64, f64)| a.1 + (b.1 - a.1) * (s - a.0) / (b.0 - a.0);
    let mut guesses: Vec<f64> = Vec::new();
    if before.is_empty() || after.is_empty() {
        match before.iter().chain(&after).copied().collect::<Vec<_>>()[..] {
            [a, b] => guesses.push(through(a, b)),
            [a] => guesses.push(a.1),
            _ => {}
        }
    } else {
        for &a in &before {
            for &b in &after {
                guesses.push(through(a, b));
            }
        }
    }
    median(&mut guesses)
}

fn median(values: &mut [f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let mid = values.len() / 2;
    Some(if values.len().is_multiple_of(2) {
        (values[mid - 1] + values[mid]) / 2.0
    } else {
        values[mid]
    })
}
