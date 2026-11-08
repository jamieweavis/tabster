//! The layout engine behind [`super::layout`].
//!
//! Pipeline: measure every bar (beat columns and fixed decorations) → collapse runs of empty bars
//! into multi-bar rests → break bars greedily into systems → justify each system (rest-only bars
//! stretch less) → emit primitives in paint order.
//!
//! Text widths are approximations ([`CHAR_WIDTH`] × font size per character): Rust has no font
//! access. The shell must draw fret numbers with a monospaced-digit font (SF Mono, or the system
//! font with `.monospacedDigit()`), where every digit is ≈0.6 em wide.

use std::ops::Range;

use super::{
    BarBox, BeatBox, DisplayList, Ink, LayoutOptions, Primitive, Rect, System, TextAnchor,
};
use crate::model::{
    Bar, Beat, Direction, MasterBar, Note, Score, Section, Slide, TICKS_PER_QUARTER, Track, Voice,
};

/// Approximate advance width of one character, as a fraction of the font size.
pub const CHAR_WIDTH: f32 = 0.6;

/// Ticks in a 64th note: the shortest duration that gets the base column width.
const SIXTY_FOURTH: f32 = (TICKS_PER_QUARTER / 16) as f32;

static EMPTY_BAR: Bar = Bar { voices: Vec::new() };

// Lanes above the staff, top to bottom. A system only reserves the lanes its bars use.
const L_SECTION: u8 = 1;
const L_MARKS: u8 = 1 << 1;
const L_CHORD: u8 = 1 << 2;
const L_ENDING: u8 = 1 << 3;
const L_RING: u8 = 1 << 4;
const L_PM: u8 = 1 << 5;
const L_BEND: u8 = 1 << 6;

pub(super) fn text_width(chars: usize, size: f32) -> f32 {
    chars as f32 * CHAR_WIDTH * size
}

/// All sizes, already multiplied by the zoom factor.
struct Metrics {
    s: f32,
    lines: usize,
    /// Distance between string lines.
    ls: f32,
    staff_h: f32,
    fret: f32,
    small: f32,
    margin: f32,
    clef_w: f32,
    lead: f32,
    tail: f32,
    repeat_w: f32,
    ts_size: f32,
    lane: f32,
}

impl Metrics {
    fn new(track: &Track, no_tab: bool, s: f32) -> Self {
        let lines = if track.is_percussion {
            5
        } else if track.tuning.is_empty() || (no_tab && track.tuning.len() < 4) {
            6
        } else {
            track.tuning.len()
        };
        let ls = 13.0 * s;
        let staff_h = ls * (lines.saturating_sub(1)) as f32;
        Self {
            s,
            lines,
            ls,
            staff_h,
            fret: 12.0 * s,
            small: 9.0 * s,
            margin: 16.0 * s,
            clef_w: 24.0 * s,
            lead: 8.0 * s,
            tail: 2.0 * s,
            repeat_w: 10.0 * s,
            ts_size: (staff_h * 0.42).clamp(10.0 * s, 28.0 * s),
            lane: 15.0 * s,
        }
    }

    fn time_sig_width(&self, mb: &MasterBar) -> f32 {
        let digits = digits(mb.time_signature.numerator).max(digits(mb.time_signature.denominator));
        text_width(digits, self.ts_size) + 10.0 * self.s
    }
}

fn digits(n: u8) -> usize {
    match n {
        0..=9 => 1,
        10..=99 => 2,
        _ => 3,
    }
}

/// One measured bar.
struct Prep {
    /// Distinct beat onsets across all voices, ascending. Never empty.
    onsets: Vec<u32>,
    /// Natural width of each onset's column.
    widths: Vec<f32>,
    cols: f32,
    /// Fixed (unstretched) space before the first column and after the last.
    left: f32,
    right: f32,
    time_sig: bool,
    lanes: u8,
    /// Nothing to draw on the staff (only rests): the bar gets a compact width that stretches
    /// less than busy bars when a system is justified.
    rest_only: bool,
    /// 1 = an ordinary bar; n ≥ 2 = the first of n rest-only bars collapsed into one multi-bar
    /// rest (its `cols`/`right` then cover the whole run); 0 = a later bar of such a run, laid
    /// out by the run's first bar (zero size of its own).
    run: u32,
    /// Minimum natural width, to fit the labels drawn over the bar.
    min_w: f32,
}

impl Prep {
    fn natural(&self) -> f32 {
        self.left + self.cols + self.right
    }
}

/// Stretch factor beyond which rest-only bars stop widening (busy bars take the rest).
const REST_STRETCH_CAP: f32 = 1.5;

struct TempoMark {
    bar: u32,
    position: f64,
    bpm: f64,
}

struct Ctx<'a> {
    score: &'a Score,
    track: &'a Track,
    m: Metrics,
    preps: Vec<Prep>,
    tempo: Vec<TempoMark>,
    rhythm: bool,
    /// A pitched track with nothing to show as tab (no tuning, or no note on a real string).
    no_tab: bool,
}

impl Ctx<'_> {
    fn bar(&self, i: usize) -> &Bar {
        self.track.bars.get(i).unwrap_or(&EMPTY_BAR)
    }

    /// Whether a note has a place on this staff.
    fn shown(&self, note: &Note) -> bool {
        self.track.is_percussion
            || (!self.no_tab && note.string >= 1 && usize::from(note.string) <= self.m.lines)
    }

    /// Whether the bar at `i` continues a multi-bar rest (laid out by the run's first bar).
    fn in_run(&self, i: usize) -> bool {
        self.preps.get(i).is_some_and(|p| p.run == 0)
    }

    fn tempo_in(&self, bar: usize) -> &[TempoMark] {
        let b = bar as u32;
        let lo = self.tempo.partition_point(|t| t.bar < b);
        let hi = self.tempo.partition_point(|t| t.bar <= b);
        &self.tempo[lo..hi]
    }
}

pub(super) fn layout(score: &Score, options: &LayoutOptions) -> DisplayList {
    let Some(track) = score.tracks.get(options.track as usize) else {
        return DisplayList::default();
    };
    if score.master_bars.is_empty() || options.width.is_nan() || options.width <= 0.0 {
        return DisplayList::default();
    }
    let s = if options.scale.is_finite() && options.scale > 0.0 {
        options.scale
    } else {
        1.0
    };
    let no_tab = !track.is_percussion && !has_tab(track);
    let m = Metrics::new(track, no_tab, s);
    let mut ctx = Ctx {
        score,
        track,
        m,
        preps: Vec::new(),
        tempo: tempo_marks(score),
        rhythm: options.show_rhythm,
        no_tab,
    };
    let mut preps: Vec<Prep> = (0..score.master_bars.len())
        .map(|i| measure(&ctx, i))
        .collect();
    group_rests(&ctx, &mut preps);
    fit_labels(&mut preps);
    ctx.preps = preps;

    let avail = (options.width - 2.0 * ctx.m.margin - ctx.m.clef_w).max(40.0 * s);
    let ranges = break_lines(&ctx.preps, avail);
    let mut systems = Vec::with_capacity(ranges.len());
    let mut y = 0.0;
    for (k, range) in ranges.iter().enumerate() {
        let system = emit_system(&ctx, range.clone(), y, avail, k + 1 == ranges.len());
        y = system.frame.y + system.frame.height;
        systems.push(system);
    }
    DisplayList {
        width: options.width,
        height: y,
        systems,
    }
}

/// Whether a pitched track has any note that can be drawn as tab: a real tuning (not the
/// all-zero placeholder importers give untuned instruments) and a note on one of its strings.
fn has_tab(track: &Track) -> bool {
    let n = track.tuning.len();
    if track.tuning.iter().all(|&p| p == 0) {
        return false;
    }
    track.bars.iter().any(|bar| {
        bar.voices.iter().any(|v| {
            v.beats.iter().any(|b| {
                b.notes
                    .iter()
                    .any(|note| note.string >= 1 && usize::from(note.string) <= n)
            })
        })
    })
}

/// Collapses runs of ≥ 2 rest-only bars into multi-bar rests. A run never crosses anything
/// that must stay visible: a section, repeat sign, time signature change, direction, tempo
/// change, alternate ending or double bar line (the run's first bar may carry the ones drawn
/// at a bar's start).
fn group_rests(ctx: &Ctx, preps: &mut [Prep]) {
    let mbs = &ctx.score.master_bars;
    let starts_run = |i: usize| {
        let mb = &mbs[i];
        preps[i].rest_only
            && mb.alternate_endings == 0
            && mb.directions.iter().all(|&d| !direction_label(d).1)
    };
    let joins = |j: usize| {
        let (mb, prev) = (&mbs[j], &mbs[j - 1]);
        preps[j].rest_only
            && mb.section.is_none()
            && !mb.repeat_start
            && !preps[j].time_sig
            && mb.directions.is_empty()
            && mb.alternate_endings == 0
            && ctx.tempo_in(j).is_empty()
            && prev.repeat_end == 0
            && !prev.double_bar
    };
    let mut runs = Vec::new();
    let mut i = 0;
    while i < preps.len() {
        let mut j = i + 1;
        if starts_run(i) {
            while j < preps.len() && joins(j) {
                j += 1;
            }
        }
        if j - i >= 2 {
            runs.push(i..j);
        }
        i = j;
    }
    let s = ctx.m.s;
    for r in runs {
        let n = r.len();
        let right = preps[r.end - 1].right;
        for p in &mut preps[r.start + 1..r.end] {
            p.run = 0;
            p.left = 0.0;
            p.cols = 0.0;
            p.right = 0.0;
        }
        let first = &mut preps[r.start];
        first.run = n as u32;
        first.right = right;
        first.cols = (44.0 + 7.0 * digits_usize(n) as f32) * s;
    }
}

fn digits_usize(n: usize) -> usize {
    n.max(1).ilog10() as usize + 1
}

/// Tempo markings to draw: the initial tempo at bar 0, then every change of value.
fn tempo_marks(score: &Score) -> Vec<TempoMark> {
    let mut marks = vec![TempoMark {
        bar: 0,
        position: 0.0,
        bpm: score.initial_tempo,
    }];
    for c in &score.tempo_changes {
        if c.bar == 0 && c.position <= 0.0 {
            marks[0].bpm = c.bpm;
            continue;
        }
        let last = marks.last().map_or(f64::NAN, |m| m.bpm);
        if (c.bpm - last).abs() >= 0.5 {
            marks.push(TempoMark {
                bar: c.bar,
                position: c.position.clamp(0.0, 1.0),
                bpm: c.bpm,
            });
        }
    }
    if marks[0].bpm.is_nan() || marks[0].bpm <= 0.0 {
        marks.remove(0);
    }
    marks
}

/// The voice whose rhythm is drawn and whose techniques are shown: voice 0, or the first
/// non-empty voice when voice 0 is empty.
fn primary_voice(bar: &Bar) -> Option<(usize, &Voice)> {
    bar.voices
        .iter()
        .enumerate()
        .find(|(_, v)| !v.beats.is_empty())
}

fn measure(ctx: &Ctx, i: usize) -> Prep {
    let m = &ctx.m;
    let mb = &ctx.score.master_bars[i];
    let bar = ctx.bar(i);

    let mut onsets = Vec::with_capacity(16);
    for voice in &bar.voices {
        let mut t = 0;
        for beat in &voice.beats {
            onsets.push(t);
            t += beat.duration.ticks();
        }
    }
    onsets.sort_unstable();
    onsets.dedup();
    if onsets.is_empty() {
        onsets.push(0);
    }
    let actual = bar.ticks();
    let ticks = if actual == 0 {
        mb.time_signature.bar_ticks()
    } else {
        actual
    };

    let rest_only = !bar.voices.iter().any(|v| {
        v.beats
            .iter()
            .any(|b| b.chord.is_some() || b.text.is_some() || b.notes.iter().any(|n| ctx.shown(n)))
    });
    // Rest-only bars are compact: a whole-bar rest needs little room.
    let (base, per_halving) = if rest_only { (8.0, 3.0) } else { (14.0, 6.0) };
    let mut widths: Vec<f32> = onsets
        .iter()
        .enumerate()
        .map(|(k, &t)| {
            let next = onsets.get(k + 1).copied().unwrap_or(ticks.max(t + 1));
            let halvings = ((next - t) as f32 / SIXTY_FOURTH).log2().max(0.0);
            (base + per_halving * halvings) * m.s
        })
        .collect();

    // Labels drawn from the bar's start must fit over it (rest-only bars are narrow).
    let mut min_w: f32 = 0.0;
    let mut lanes = 0;
    if let Some(sec) = &mb.section {
        lanes |= L_SECTION;
        min_w = text_width(section_label_len(sec), 11.0 * m.s) + 12.0 * m.s;
    }
    if !mb.directions.is_empty() || !ctx.tempo_in(i).is_empty() {
        lanes |= L_MARKS;
        let tempo = ctx.tempo_in(i).len() * TEMPO_LABEL_CHARS;
        let dirs: usize = mb
            .directions
            .iter()
            .map(|&d| direction_label(d).0.chars().count() + 2)
            .sum();
        min_w = min_w.max(text_width(tempo + dirs, 10.0 * m.s) + 12.0 * m.s);
    }
    if mb.alternate_endings != 0 {
        lanes |= L_ENDING;
    }
    if i == 0 && (ctx.track.is_percussion || ctx.no_tab) {
        lanes |= L_CHORD;
        min_w = min_w.max(text_width(NO_TAB_LABEL.len(), 10.0 * m.s) + 12.0 * m.s);
    }
    let primary = primary_voice(bar).map(|(v, _)| v);
    for (v, voice) in bar.voices.iter().enumerate() {
        let mut t = 0;
        for beat in &voice.beats {
            let col = onsets.binary_search(&t).unwrap_or(0);
            let onset = t;
            t += beat.duration.ticks();
            if beat.chord.is_some() || beat.text.is_some() {
                lanes |= L_CHORD;
            }
            if ctx.track.is_percussion {
                continue;
            }
            for note in &beat.notes {
                if !ctx.shown(note) {
                    continue;
                }
                // A tied note is only written out (as "(n)") at the start of a system, which
                // can only be a bar's first beat; elsewhere a tie arc replaces it.
                let label = if note.tie_destination && onset > 0 {
                    0
                } else {
                    label_len(note)
                };
                let mut w = text_width(label, m.fret) + 6.0 * m.s;
                if note.effects.bend.is_some() {
                    w += 10.0 * m.s;
                }
                if note.effects.slide.is_some() {
                    w += 4.0 * m.s;
                }
                widths[col] = widths[col].max(w);
                if Some(v) == primary {
                    let e = &note.effects;
                    if e.let_ring {
                        lanes |= L_RING;
                    }
                    if e.palm_mute {
                        lanes |= L_PM;
                    }
                    if e.bend.is_some() || e.vibrato {
                        lanes |= L_BEND;
                    }
                }
            }
        }
    }

    let time_sig = i == 0 || ctx.score.master_bars[i - 1].time_signature != mb.time_signature;
    let mut left = m.lead;
    if time_sig {
        left += m.time_sig_width(mb);
    }
    if mb.repeat_start {
        left += m.repeat_w;
    }
    let mut right = m.tail;
    if mb.repeat_end > 0 {
        right += m.repeat_w;
    }
    let cols = widths.iter().sum();
    Prep {
        onsets,
        widths,
        cols,
        left,
        right,
        time_sig,
        lanes,
        rest_only,
        run: 1,
        min_w,
    }
}

const NO_TAB_LABEL: &str = "No tab for this track";

/// Approximate length of a "♩ = 120" tempo label plus spacing.
const TEMPO_LABEL_CHARS: usize = 9;

fn section_label_len(sec: &Section) -> usize {
    let (l, t) = (sec.letter.chars().count(), sec.text.chars().count());
    match (l, t) {
        (0, t) => t,
        (l, 0) => l,
        (l, t) => l + 3 + t,
    }
}

/// Widens bars (and multi-bar rests) too narrow for the labels drawn over them.
fn fit_labels(preps: &mut [Prep]) {
    for p in preps.iter_mut().filter(|p| p.run != 0) {
        let extra = p.min_w - p.natural();
        if extra <= 0.0 {
            continue;
        }
        if p.run == 1 && p.cols > 0.0 {
            let k = (p.cols + extra) / p.cols;
            p.widths.iter_mut().for_each(|w| *w *= k);
        }
        p.cols += extra;
    }
}

/// Greedy line breaking: as many bars per system as fit; a bar always starts a system if it
/// doesn't fit next to others. A multi-bar rest is never split.
fn break_lines(preps: &[Prep], avail: f32) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut start = 0;
    let mut acc = 0.0;
    for (i, p) in preps.iter().enumerate() {
        let w = p.natural();
        if i > start && p.run != 0 && acc + w > avail {
            ranges.push(start..i);
            start = i;
            acc = 0.0;
        }
        acc += w;
    }
    ranges.push(start..preps.len());
    ranges
}

/// Vertical positions within one system.
struct Rows {
    section: f32,
    marks: f32,
    chord: f32,
    ending: f32,
    ring: f32,
    pm: f32,
    bend: f32,
    staff_top: f32,
    staff_bot: f32,
    stem_top: f32,
    stem_bot: f32,
    bottom: f32,
}

impl Rows {
    fn new(m: &Metrics, top: f32, lanes: u8, rhythm: bool, perc: bool) -> Self {
        let s = m.s;
        let mut y = top + 6.0 * s;
        let mut lane = |flag: u8, h: f32| {
            let at = y;
            if lanes & flag != 0 {
                y += h;
            }
            at
        };
        let section = lane(L_SECTION, 20.0 * s);
        let marks = lane(L_MARKS, m.lane);
        let chord = lane(L_CHORD, m.lane);
        let ending = lane(L_ENDING, m.lane);
        let ring = lane(L_RING, m.lane);
        let pm = lane(L_PM, m.lane);
        let bend = lane(L_BEND, m.lane);
        // Drum staves have cymbal heads above the top line (and on a ledger line).
        let staff_top = y + if perc { 26.0 } else { 14.0 } * s;
        let staff_bot = staff_top + m.staff_h;
        let (stem_top, stem_bot, bottom) = if rhythm {
            let stem_top = staff_bot + 10.0 * s;
            let stem_bot = stem_top + 22.0 * s;
            (stem_top, stem_bot, stem_bot + 22.0 * s)
        } else {
            (staff_bot, staff_bot, staff_bot + 22.0 * s)
        };
        Self {
            section,
            marks,
            chord,
            ending,
            ring,
            pm,
            bend,
            staff_top,
            staff_bot,
            stem_top,
            stem_bot,
            bottom,
        }
    }

    fn string_y(&self, m: &Metrics, string: u8) -> f32 {
        self.staff_top + f32::from(string.saturating_sub(1)) * m.ls
    }
}

/// Primitive sink with terse helpers.
struct Out {
    p: Vec<Primitive>,
}

impl Out {
    fn line(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, width: f32, ink: Ink) {
        self.p.push(Primitive::Line {
            x1,
            y1,
            x2,
            y2,
            width,
            ink,
        });
    }

    fn rect(&mut self, x: f32, y: f32, width: f32, height: f32, ink: Ink) {
        self.p.push(Primitive::Rect {
            rect: Rect {
                x,
                y,
                width,
                height,
            },
            ink,
        });
    }

    fn knockout(&mut self, cx: f32, cy: f32, width: f32, height: f32) {
        self.p.push(Primitive::Knockout {
            rect: Rect {
                x: cx - width / 2.0,
                y: cy - height / 2.0,
                width,
                height,
            },
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn text(
        &mut self,
        x: f32,
        y: f32,
        text: impl Into<String>,
        size: f32,
        style: (bool, bool),
        anchor: TextAnchor,
        ink: Ink,
    ) {
        self.p.push(Primitive::Text {
            x,
            y,
            text: text.into(),
            size,
            bold: style.0,
            italic: style.1,
            anchor,
            ink,
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn curve(&mut self, x1: f32, y1: f32, cx: f32, cy: f32, x2: f32, y2: f32, width: f32) {
        self.p.push(Primitive::Curve {
            x1,
            y1,
            cx,
            cy,
            x2,
            y2,
            width,
            ink: Ink::Primary,
        });
    }

    fn dot(&mut self, x: f32, y: f32, radius: f32, ink: Ink) {
        self.p.push(Primitive::Dot { x, y, radius, ink });
    }
}

const PLAIN: (bool, bool) = (false, false);
const BOLD: (bool, bool) = (true, false);
const ITALIC: (bool, bool) = (false, true);

fn emit_system(ctx: &Ctx, range: Range<usize>, top: f32, avail: f32, last: bool) -> System {
    let m = &ctx.m;
    let preps = &ctx.preps[range.clone()];
    let lanes = preps.iter().fold(0, |acc, p| acc | p.lanes);
    let rows = Rows::new(m, top, lanes, ctx.rhythm, ctx.track.is_percussion);

    // Justify: stretch (or compress) the beat columns; fixed decorations keep their size.
    let mut fixed = 0.0;
    let (mut busy, mut rests) = (0.0, 0.0);
    for p in preps {
        fixed += p.left + p.right;
        if p.rest_only {
            rests += p.cols;
        } else {
            busy += p.cols;
        }
    }
    let (f, fr) = justify(avail - fixed, busy, rests, last);

    let frame_y = rows.staff_top - m.ls / 2.0;
    let frame_h = m.staff_h + m.ls;
    let frame = |x: f32, right: f32| Rect {
        x,
        y: frame_y,
        width: right - x,
        height: frame_h,
    };
    let mut x = m.margin + m.clef_w;
    let mut bars: Vec<BarBox> = Vec::with_capacity(range.len());
    for (i, p) in range.clone().zip(preps) {
        match p.run {
            0 => {} // Laid out with its multi-bar rest.
            1 => {
                let f = if p.rest_only { fr } else { f };
                let mut cx = x + p.left;
                let beats = p
                    .onsets
                    .iter()
                    .zip(&p.widths)
                    .map(|(&t, &w)| {
                        let w = w * f;
                        let b = BeatBox {
                            tick_in_bar: t,
                            x: cx + w / 2.0,
                        };
                        cx += w;
                        b
                    })
                    .collect();
                let right = cx + p.right;
                bars.push(BarBox {
                    master_bar: i as u32,
                    frame: frame(x, right),
                    beats,
                });
                x = right;
            }
            n => {
                // One slice of the multi-rest per bar, so the cursor glides through it bar by
                // bar; each slice's only onset sits at its left edge.
                let n = n as usize;
                let content = x + p.left;
                let per = p.cols * fr / n as f32;
                let nudge = (per * 0.05).min(0.5 * m.s);
                for k in 0..n {
                    let (x0, onset) = if k == 0 {
                        (x, content)
                    } else {
                        let x0 = content + k as f32 * per;
                        (x0, x0 + nudge)
                    };
                    let mut x1 = content + (k + 1) as f32 * per;
                    if k + 1 == n {
                        x1 += p.right;
                    }
                    bars.push(BarBox {
                        master_bar: (i + k) as u32,
                        frame: frame(x0, x1),
                        beats: vec![BeatBox {
                            tick_in_bar: 0,
                            x: onset,
                        }],
                    });
                    x = x1;
                }
            }
        }
    }
    let right = x;

    let mut out = Out {
        p: Vec::with_capacity(preps.len() * 64),
    };
    let mut sys = SystemEmitter {
        ctx,
        m,
        rows: &rows,
        bars: &bars,
        out: &mut out,
    };
    sys.staff(right);
    for k in 0..bars.len() {
        sys.bar(k);
    }
    sys.endings();
    sys.spans();

    System {
        frame: Rect {
            x: 0.0,
            y: top,
            width: (right + m.margin).max(avail + 2.0 * m.margin + m.clef_w),
            height: rows.bottom - top,
        },
        primitives: out.p,
        bars,
    }
}

/// Stretch factors (busy bars, rest-only bars) that fill `space` with columns of natural
/// widths `busy` and `rests`. Rest-only bars stretch like the others up to
/// [`REST_STRETCH_CAP`]; beyond that only busy bars widen, so a few empty bars don't push the
/// music around. The last system is never stretched much.
fn justify(space: f32, busy: f32, rests: f32, last: bool) -> (f32, f32) {
    let (mut f, mut fr) = if busy <= 0.0 {
        let fr = if rests > 0.0 { space / rests } else { 1.0 };
        (fr, fr)
    } else {
        let f = (space - rests * REST_STRETCH_CAP) / busy;
        if f >= REST_STRETCH_CAP {
            (f, REST_STRETCH_CAP)
        } else {
            let u = space / (busy + rests);
            (u, u)
        }
    };
    if last {
        f = f.min(1.3);
        fr = fr.min(1.3);
    }
    (f.max(0.2), fr.max(0.2))
}

struct SystemEmitter<'a> {
    ctx: &'a Ctx<'a>,
    m: &'a Metrics,
    rows: &'a Rows,
    bars: &'a [BarBox],
    out: &'a mut Out,
}

impl SystemEmitter<'_> {
    fn staff(&mut self, right: f32) {
        let (m, r) = (self.m, self.rows);
        let s = m.s;
        let left = m.margin;
        for k in 0..m.lines {
            let y = r.staff_top + k as f32 * m.ls;
            self.out.line(left, y, right, y, 0.7 * s, Ink::Secondary);
        }
        self.out
            .line(left, r.staff_top, left, r.staff_bot, 1.0 * s, Ink::Primary);

        // Clef.
        let cx = left + m.clef_w / 2.0;
        if self.ctx.track.is_percussion {
            let h = m.staff_h * 0.4;
            let y = r.staff_top + (m.staff_h - h) / 2.0;
            self.out.rect(cx - 4.0 * s, y, 2.5 * s, h, Ink::Primary);
            self.out.rect(cx + 1.5 * s, y, 2.5 * s, h, Ink::Primary);
        } else if m.lines >= 3 {
            let size = (m.staff_h / 3.6).min(15.0 * s);
            for (k, letter) in ["T", "A", "B"].into_iter().enumerate() {
                let y = r.staff_top + m.staff_h * (0.5 + (k as f32 - 1.0) * 0.32);
                self.out.knockout(cx, y, size * 0.8, size);
                self.out
                    .text(cx, y, letter, size, BOLD, TextAnchor::Center, Ink::Primary);
            }
        }

        // Bar number.
        if let Some(bar) = self.bars.first() {
            self.out.text(
                left,
                r.staff_top - 5.0 * s,
                (bar.master_bar + 1).to_string(),
                m.small,
                PLAIN,
                TextAnchor::Leading,
                Ink::Secondary,
            );
        }
    }

    fn bar(&mut self, k: usize) {
        let (ctx, m) = (self.ctx, self.m);
        let s = m.s;
        let bb = &self.bars[k];
        let i = bb.master_bar as usize;
        let mb = &ctx.score.master_bars[i];
        let prep = &ctx.preps[i];
        let r = self.rows;
        let bx = bb.frame.x;
        let bright = bx + bb.frame.width;

        self.barlines(k);
        if prep.run == 0 {
            return;
        }

        // Time signature.
        if prep.time_sig {
            let rs = if mb.repeat_start { m.repeat_w } else { 0.0 };
            let w = m.time_sig_width(mb);
            let cx = bx + rs + m.lead / 2.0 + w / 2.0;
            let mid = r.staff_top + m.staff_h / 2.0;
            let half = (m.staff_h / 4.0).max(m.ts_size * 0.5);
            self.out.knockout(cx, mid, w - 4.0 * s, m.staff_h + 2.0 * s);
            let ts = mb.time_signature;
            for (n, y) in [(ts.numerator, mid - half), (ts.denominator, mid + half)] {
                self.out.text(
                    cx,
                    y,
                    n.to_string(),
                    m.ts_size,
                    BOLD,
                    TextAnchor::Center,
                    Ink::Primary,
                );
            }
        }

        // Section label.
        if let Some(sec) = &mb.section {
            let label = match (sec.letter.is_empty(), sec.text.is_empty()) {
                (false, false) => format!("{} · {}", sec.letter, sec.text),
                (true, _) => sec.text.clone(),
                (false, true) => sec.letter.clone(),
            };
            self.out.text(
                bx + 2.0 * s,
                r.section + 15.0 * s,
                label,
                11.0 * s,
                BOLD,
                TextAnchor::Leading,
                Ink::Accent,
            );
        }

        // Tempo and navigation marks.
        let base = r.marks + 11.0 * s;
        let mut tx = bx + 2.0 * s;
        let first_x = bb.beats.first().map_or(bx, |b| b.x);
        for t in ctx.tempo_in(i) {
            let x = if t.position > 0.0 {
                first_x + (bright - first_x) * t.position as f32
            } else {
                tx
            };
            let label = format!("♩ = {}", t.bpm.round() as i64);
            let w = text_width(label.chars().count(), 10.0 * s);
            self.out.text(
                x,
                base,
                label,
                10.0 * s,
                BOLD,
                TextAnchor::Leading,
                Ink::Primary,
            );
            tx = tx.max(x + w + 8.0 * s);
        }
        let mut jumps_w = 0.0;
        for &d in &mb.directions {
            let (label, at_end) = direction_label(d);
            let w = text_width(label.chars().count(), 10.0 * s);
            if at_end {
                jumps_w += w + 8.0 * s;
                let x = bright - jumps_w + 8.0 * s - 4.0 * s;
                self.out.text(
                    x,
                    base,
                    label,
                    10.0 * s,
                    ITALIC,
                    TextAnchor::Leading,
                    Ink::Secondary,
                );
            } else {
                self.out.text(
                    tx,
                    base,
                    label,
                    10.0 * s,
                    ITALIC,
                    TextAnchor::Leading,
                    Ink::Secondary,
                );
                tx += w + 8.0 * s;
            }
        }

        let label = if ctx.track.is_percussion {
            Some("Percussion")
        } else if ctx.no_tab {
            Some(NO_TAB_LABEL)
        } else {
            None
        };
        if let Some(label) = label.filter(|_| i == 0) {
            self.out.text(
                bx + 2.0 * s,
                r.chord + 11.0 * s,
                label,
                10.0 * s,
                ITALIC,
                TextAnchor::Leading,
                Ink::Secondary,
            );
        }

        if prep.run >= 2 {
            let last = &self.bars[k + prep.run as usize - 1];
            let x0 = bb.beats.first().map_or(bx, |b| b.x);
            let x1 = last.frame.x + last.frame.width - prep.right;
            self.multi_rest(x0, x1, prep.run);
            return;
        }
        if ctx.track.is_percussion {
            self.drums(k);
        } else {
            self.notes(k);
        }
        if ctx.rhythm {
            self.rhythm(k);
        }
    }

    /// A multi-bar rest spanning `x0..x1`: a thick bar with end serifs and the bar count.
    fn multi_rest(&mut self, x0: f32, x1: f32, count: u32) {
        let (m, r) = (self.m, self.rows);
        let s = m.s;
        let (x0, x1) = (x0 + 4.0 * s, x1 - 6.0 * s);
        let mx = (x0 + x1) / 2.0;
        let (y, label_y) = if self.ctx.rhythm {
            let y = r.stem_top + 15.0 * s;
            (y, y - 11.0 * s)
        } else {
            let y = r.staff_top + m.staff_h / 2.0;
            self.out
                .knockout(mx, y - 4.0 * s, x1 - x0 + 4.0 * s, 24.0 * s);
            (y, y - 11.0 * s)
        };
        self.out
            .rect(x0, y - 2.5 * s, x1 - x0, 5.0 * s, Ink::Primary);
        for x in [x0, x1] {
            self.out
                .line(x, y - 5.0 * s, x, y + 5.0 * s, 1.2 * s, Ink::Primary);
        }
        self.out.text(
            mx,
            label_y,
            count.to_string(),
            12.0 * s,
            BOLD,
            TextAnchor::Center,
            Ink::Primary,
        );
    }

    /// Drum notation on the 5-line staff: each hit placed by instrument, cymbals and hi-hats
    /// as "x", drums as filled heads.
    fn drums(&mut self, k: usize) {
        let (ctx, m, r) = (self.ctx, self.m, self.rows);
        let s = m.s;
        let bb = &self.bars[k];
        let bar = ctx.bar(bb.master_bar as usize);
        let half = m.ls / 2.0;
        for voice in &bar.voices {
            let mut t = 0;
            for beat in &voice.beats {
                let x = x_at(bb, t);
                t += beat.duration.ticks();
                for note in &beat.notes {
                    let key = ctx.track.pitch(note.string, note.fret).unwrap_or(note.fret);
                    let (pos, cymbal) = drum_position(key);
                    let y = r.staff_top + f32::from(pos) * half;
                    // Ledger lines for heads above the staff's first ledger position.
                    let mut ledger = -2;
                    while ledger >= pos {
                        let ly = r.staff_top + f32::from(ledger) * half;
                        self.out
                            .line(x - 7.0 * s, ly, x + 7.0 * s, ly, 0.8 * s, Ink::Secondary);
                        ledger -= 2;
                    }
                    let ink = if note.effects.ghost {
                        Ink::Secondary
                    } else {
                        Ink::Primary
                    };
                    if cymbal {
                        self.out
                            .text(x, y, "x", 12.0 * s, BOLD, TextAnchor::Center, ink);
                    } else {
                        self.out.dot(x, y, 3.6 * s, ink);
                    }
                }
            }
        }
    }

    fn barlines(&mut self, k: usize) {
        let (ctx, m, r) = (self.ctx, self.m, self.rows);
        let s = m.s;
        let bb = &self.bars[k];
        let i = bb.master_bar as usize;
        let mb = &ctx.score.master_bars[i];
        let (top, bot) = (r.staff_top, r.staff_bot);
        let mid = (top + bot) / 2.0;
        let dot_dy = if m.lines % 2 == 0 { m.ls } else { m.ls / 2.0 };
        let dot_dy = if m.lines < 3 { 0.0 } else { dot_dy };
        let thin = 1.0 * s;
        let thick = 3.0 * s;
        let bx = bb.frame.x;
        let right = bx + bb.frame.width;

        if mb.repeat_start {
            self.out
                .line(bx + 1.5 * s, top, bx + 1.5 * s, bot, thick, Ink::Primary);
            self.out
                .line(bx + 5.0 * s, top, bx + 5.0 * s, bot, thin, Ink::Primary);
            for dy in [-dot_dy, dot_dy] {
                self.out.dot(bx + 8.5 * s, mid + dy, 1.6 * s, Ink::Primary);
            }
        }

        // Repeat count.
        if mb.repeat_end > 2 {
            let label = format!("x{}", mb.repeat_end);
            let w = text_width(label.len(), 10.0 * s);
            self.out.text(
                right - w - 2.0 * s,
                r.staff_top - 5.0 * s,
                label,
                10.0 * s,
                BOLD,
                TextAnchor::Leading,
                Ink::Primary,
            );
        }

        // No bar lines inside a multi-bar rest.
        if ctx.in_run(i + 1) {
            return;
        }
        let final_bar = i + 1 == ctx.score.master_bars.len();
        if mb.repeat_end > 0 {
            for dy in [-dot_dy, dot_dy] {
                self.out
                    .dot(right - 8.5 * s, mid + dy, 1.6 * s, Ink::Primary);
            }
        }
        if mb.repeat_end > 0 || final_bar {
            self.out.line(
                right - 5.0 * s,
                top,
                right - 5.0 * s,
                bot,
                thin,
                Ink::Primary,
            );
            self.out.line(
                right - 1.5 * s,
                top,
                right - 1.5 * s,
                bot,
                thick,
                Ink::Primary,
            );
        } else if mb.double_bar {
            self.out.line(
                right - 3.5 * s,
                top,
                right - 3.5 * s,
                bot,
                thin,
                Ink::Primary,
            );
            self.out.line(right, top, right, bot, thin, Ink::Primary);
        } else {
            self.out.line(right, top, right, bot, thin, Ink::Primary);
        }
    }

    fn notes(&mut self, k: usize) {
        let (ctx, m, r) = (self.ctx, self.m, self.rows);
        let s = m.s;
        let bb = &self.bars[k];
        let i = bb.master_bar as usize;
        let bar = ctx.bar(i);
        let primary = primary_voice(bar).map(|(v, _)| v);
        let knock_h = m.fret * 0.95;

        // Fret numbers for every voice. A tied note continues the previous note on its string:
        // on the same system it's a tie arc from that note (no number); at the start of a
        // system it's written as "(n)".
        for (v, voice) in bar.voices.iter().enumerate() {
            // The previous beat in this voice: x, beat, and whether it starts the system.
            let mut prev = k.checked_sub(1).and_then(|pk| {
                last_beat(ctx, &self.bars[pk], v).map(|(x, b, n)| (x, b, pk == 0 && n == 1))
            });
            let mut t = 0;
            for (j, beat) in voice.beats.iter().enumerate() {
                let x = x_at(bb, t);
                t += beat.duration.ticks();
                for note in &beat.notes {
                    if !ctx.shown(note) {
                        continue;
                    }
                    let y = r.string_y(m, note.string);
                    let origin =
                        prev.filter(|_| note.tie_destination)
                            .and_then(|(px, pb, start)| {
                                let on = pb.notes.iter().find(|n| n.string == note.string)?;
                                Some((px, on, start))
                            });
                    if let Some((px, on, start)) = origin {
                        let drawn = !on.tie_destination || start;
                        let x1 = if drawn {
                            px + text_width(label_len(on), m.fret) / 2.0 + 1.0 * s
                        } else {
                            px
                        };
                        let x2 = x - 1.0 * s;
                        self.out.curve(
                            x1,
                            y + 4.0 * s,
                            (x1 + x2) / 2.0,
                            y + 10.0 * s,
                            x2,
                            y + 4.0 * s,
                            1.0 * s,
                        );
                        continue;
                    }
                    let label = note_label(note);
                    let w = text_width(label.chars().count(), m.fret);
                    self.out.knockout(x, y, w + 2.0 * s, knock_h);
                    let (ink, style) = if note.effects.ghost || note.tie_destination {
                        (Ink::Secondary, PLAIN)
                    } else {
                        (Ink::Primary, BOLD)
                    };
                    self.out
                        .text(x, y, label, m.fret, style, TextAnchor::Center, ink);
                }
                prev = Some((x, beat, k == 0 && j == 0));
            }
            // Ties into the next system: an open arc to the end of the staff.
            if k + 1 == self.bars.len()
                && let Some((px, pb, start)) = prev
                && let Some(next) = ctx.bar(i + 1).voices.get(v).and_then(|nv| nv.beats.first())
            {
                let x2 = bb.frame.x + bb.frame.width + 2.0 * s;
                for note in next.notes.iter().filter(|n| n.tie_destination) {
                    let Some(on) = pb
                        .notes
                        .iter()
                        .find(|n| n.string == note.string && ctx.shown(n))
                    else {
                        continue;
                    };
                    let y = r.string_y(m, note.string);
                    let x1 = if !on.tie_destination || start {
                        px + text_width(label_len(on), m.fret) / 2.0 + 1.0 * s
                    } else {
                        px
                    };
                    if x2 > x1 {
                        self.out.curve(
                            x1,
                            y + 4.0 * s,
                            (x1 + x2) / 2.0,
                            y + 10.0 * s,
                            x2,
                            y + 6.0 * s,
                            1.0 * s,
                        );
                    }
                }
            }
        }

        // Techniques, chords and beat text for the primary voice.
        let Some(v) = primary else { return };
        let voice = &bar.voices[v];
        let mut t = 0;
        for (j, beat) in voice.beats.iter().enumerate() {
            let x = x_at(bb, t);
            t += beat.duration.ticks();
            let next = Self::next_beat(ctx, self.bars, (k, v, j, t));
            if let Some(chord) = &beat.chord {
                self.out.text(
                    x,
                    r.chord + 8.0 * s,
                    chord.as_str(),
                    10.0 * s,
                    BOLD,
                    TextAnchor::Center,
                    Ink::Primary,
                );
            }
            if let Some(text) = &beat.text {
                let x = if beat.chord.is_some() {
                    x + 10.0 * s
                } else {
                    x
                };
                self.out.text(
                    x,
                    r.chord + 11.0 * s,
                    text.as_str(),
                    10.0 * s,
                    ITALIC,
                    TextAnchor::Leading,
                    Ink::Secondary,
                );
            }
            let (mut vibrato, mut bend) = (false, false);
            for note in &beat.notes {
                if !ctx.shown(note) {
                    continue;
                }
                vibrato |= note.effects.vibrato;
                bend |= note.effects.bend.is_some();
                self.techniques(x, beat, note, next);
            }
            if vibrato {
                // After the bend label when the beat also bends.
                let vx = if bend { x + 26.0 * s } else { x - 3.0 * s };
                self.out.text(
                    vx,
                    r.bend + 11.0 * s,
                    "~~~",
                    11.0 * s,
                    BOLD,
                    TextAnchor::Leading,
                    Ink::Primary,
                );
            }
        }
    }

    /// The beat after beat `j` of voice `v` in system bar `k` (possibly the first beat of the
    /// next bar, if that bar is in this system), with its x.
    fn next_beat<'b>(
        ctx: &'b Ctx,
        bars: &'b [BarBox],
        (k, v, j, tick): (usize, usize, usize, u32),
    ) -> Option<(f32, &'b Beat)> {
        let bb = &bars[k];
        let bar = ctx.bar(bb.master_bar as usize);
        if let Some(b) = bar.voices[v].beats.get(j + 1) {
            return Some((x_at(bb, tick), b));
        }
        let nb = bars.get(k + 1)?;
        let beat = ctx
            .bar(nb.master_bar as usize)
            .voices
            .get(v)?
            .beats
            .first()?;
        Some((x_at(nb, 0), beat))
    }

    fn techniques(&mut self, x: f32, _beat: &Beat, note: &Note, next: Option<(f32, &Beat)>) {
        let (m, r) = (self.m, self.rows);
        let s = m.s;
        let y = r.string_y(m, note.string);
        let hw = text_width(label_len(note), m.fret) / 2.0;
        let target = next.and_then(|(nx, b)| {
            b.notes
                .iter()
                .find(|n| n.string == note.string)
                .map(|n| (nx, n))
        });
        let e = &note.effects;

        if e.hammer_pull {
            let (x2, label) = match target {
                Some((nx, n)) => (nx, if n.fret < note.fret { "P" } else { "H" }),
                None => (x + 16.0 * s, "H"),
            };
            let mx = (x + x2) / 2.0;
            self.out.curve(
                x + 2.0 * s,
                y - 6.0 * s,
                mx,
                y - 13.0 * s,
                x2 - 2.0 * s,
                y - 6.0 * s,
                0.9 * s,
            );
            self.out.knockout(mx, y - 14.5 * s, 6.0 * s, 7.0 * s);
            self.out.text(
                mx,
                y - 14.5 * s,
                label,
                7.5 * s,
                PLAIN,
                TextAnchor::Center,
                Ink::Secondary,
            );
        }

        if let Some(slide) = e.slide {
            let x1 = x + hw + 2.0 * s;
            let d = 3.0 * s;
            match (slide, target) {
                (Slide::Legato | Slide::Shift, Some((nx, n))) => {
                    let x2 = nx - text_width(label_len(n), m.fret) / 2.0 - 2.0 * s;
                    let dy = if n.fret >= note.fret { d } else { -d };
                    self.out.line(x1, y + dy, x2, y - dy, 1.0 * s, Ink::Primary);
                }
                (Slide::Legato | Slide::Shift | Slide::OutDown, _) => {
                    self.out
                        .line(x1, y - d, x1 + 9.0 * s, y + d, 1.0 * s, Ink::Primary);
                }
                (Slide::OutUp, _) => {
                    self.out
                        .line(x1, y + d, x1 + 9.0 * s, y - d, 1.0 * s, Ink::Primary);
                }
                (Slide::InFromBelow, _) => {
                    let x2 = x - hw - 2.0 * s;
                    self.out
                        .line(x2 - 9.0 * s, y + d, x2, y - d, 1.0 * s, Ink::Primary);
                }
                (Slide::InFromAbove, _) => {
                    let x2 = x - hw - 2.0 * s;
                    self.out
                        .line(x2 - 9.0 * s, y - d, x2, y + d, 1.0 * s, Ink::Primary);
                }
            }
        }

        if let Some(bend) = &e.bend {
            let amount = bend.points.iter().map(|p| p.1).fold(0.0f32, f32::max);
            let x1 = x + hw + 1.0 * s;
            let tip_x = x1 + 8.0 * s;
            let tip_y = r.bend + m.lane - 1.0 * s;
            self.out.curve(x1, y, tip_x, y, tip_x, tip_y, 1.0 * s);
            let a = 2.5 * s;
            self.out.line(
                tip_x,
                tip_y,
                tip_x - a,
                tip_y + 1.6 * a,
                1.0 * s,
                Ink::Primary,
            );
            self.out.line(
                tip_x,
                tip_y,
                tip_x + a,
                tip_y + 1.6 * a,
                1.0 * s,
                Ink::Primary,
            );
            self.out.text(
                tip_x,
                r.bend + 6.0 * s,
                bend_label(amount),
                9.0 * s,
                PLAIN,
                TextAnchor::Center,
                Ink::Primary,
            );
        }
    }

    /// Alternate-ending brackets.
    fn endings(&mut self) {
        let (ctx, m, r) = (self.ctx, self.m, self.rows);
        let s = m.s;
        let mut cur: Option<(u8, f32, f32, bool)> = None;
        let flush = |out: &mut Out, span: (u8, f32, f32, bool)| {
            let (mask, x1, x2, closed) = span;
            let top = r.ending + 3.0 * s;
            let bot = r.ending + m.lane;
            out.line(x1 + 2.0 * s, bot, x1 + 2.0 * s, top, 1.0 * s, Ink::Primary);
            out.line(x1 + 2.0 * s, top, x2 - 3.0 * s, top, 1.0 * s, Ink::Primary);
            if closed {
                out.line(x2 - 3.0 * s, top, x2 - 3.0 * s, bot, 1.0 * s, Ink::Primary);
            }
            let label: Vec<String> = (0..8)
                .filter(|b| mask & (1 << b) != 0)
                .map(|b| format!("{}.", b + 1))
                .collect();
            out.text(
                x1 + 5.0 * s,
                top + 9.5 * s,
                label.join(" "),
                10.0 * s,
                PLAIN,
                TextAnchor::Leading,
                Ink::Primary,
            );
        };
        for bb in self.bars {
            let mb = &ctx.score.master_bars[bb.master_bar as usize];
            let mask = mb.alternate_endings;
            let right = bb.frame.x + bb.frame.width;
            match &mut cur {
                Some(c) if c.0 == mask => {
                    c.2 = right;
                    c.3 = mb.repeat_end > 0;
                }
                _ => {
                    if let Some(c) = cur.take() {
                        flush(self.out, c);
                    }
                    if mask != 0 {
                        cur = Some((mask, bb.frame.x, right, mb.repeat_end > 0));
                    }
                }
            }
        }
        if let Some(c) = cur {
            flush(self.out, c);
        }
    }

    /// "P.M." and "let ring" spans over consecutive beats of the primary voice.
    fn spans(&mut self) {
        let (ctx, m, r) = (self.ctx, self.m, self.rows);
        let s = m.s;
        for (flag_y, label, is_pm) in [(r.ring, "let ring", false), (r.pm, "P.M.", true)] {
            let mut span: Option<(f32, f32)> = None;
            let flush = |out: &mut Out, (x1, x2): (f32, f32)| {
                let base = flag_y + 11.0 * s;
                out.text(
                    x1,
                    base,
                    label,
                    9.0 * s,
                    ITALIC,
                    TextAnchor::Leading,
                    Ink::Secondary,
                );
                let tx = x1 + text_width(label.len(), 9.0 * s) * 0.9 + 3.0 * s;
                let ly = base - 3.0 * s;
                if x2 > tx + 4.0 * s {
                    let mut dx = tx;
                    while dx < x2 {
                        let end = (dx + 3.0 * s).min(x2);
                        out.line(dx, ly, end, ly, 0.8 * s, Ink::Secondary);
                        dx += 5.0 * s;
                    }
                    out.line(x2, ly - 3.0 * s, x2, ly + 3.0 * s, 0.8 * s, Ink::Secondary);
                }
            };
            for bb in self.bars {
                let bar = ctx.bar(bb.master_bar as usize);
                let Some((_, voice)) = primary_voice(bar) else {
                    if let Some(sp) = span.take() {
                        flush(self.out, sp);
                    }
                    continue;
                };
                let mut t = 0;
                for beat in &voice.beats {
                    let x = x_at(bb, t);
                    t += beat.duration.ticks();
                    let on = beat.notes.iter().any(|n| {
                        if is_pm {
                            n.effects.palm_mute
                        } else {
                            n.effects.let_ring
                        }
                    });
                    if on {
                        let sp = span.get_or_insert((x - 4.0 * s, x));
                        sp.1 = x + 4.0 * s;
                    } else if let Some(sp) = span.take() {
                        flush(self.out, sp);
                    }
                }
            }
            if let Some(sp) = span {
                flush(self.out, sp);
            }
        }
    }

    fn rhythm(&mut self, k: usize) {
        let (ctx, m, r) = (self.ctx, self.m, self.rows);
        let s = m.s;
        let bb = &self.bars[k];
        let i = bb.master_bar as usize;
        let bar = ctx.bar(i);
        let Some((_, voice)) = primary_voice(bar) else {
            // Empty bar: a whole rest in the middle of the bar.
            let x = bb.frame.x + bb.frame.width / 2.0;
            rest(self.out, s, x, (r.stem_top + r.stem_bot) / 2.0, 1, 0);
            return;
        };
        let ts = ctx.score.master_bars[i].time_signature;
        let group = if ts.denominator >= 8 && ts.numerator % 3 == 0 {
            3 * (4 * TICKS_PER_QUARTER / u32::from(ts.denominator))
        } else {
            TICKS_PER_QUARTER
        };
        let (top, bot) = (r.stem_top, r.stem_bot);
        let mid = (top + bot) / 2.0;
        let beam_h = 3.0 * s;
        let beam_gap = 5.0 * s;

        // Stems, rests, dots.
        let mut t = 0;
        let mut run: Vec<(f32, u32, u32)> = Vec::with_capacity(8); // (x, levels, group key)
        let flush_run = |out: &mut Out, run: &mut Vec<(f32, u32, u32)>| {
            match run.len() {
                0 => {}
                1 => {
                    let (x, levels, _) = run[0];
                    for l in 0..levels {
                        let y = bot - l as f32 * beam_gap;
                        out.curve(
                            x,
                            y,
                            x + 7.0 * s,
                            y - 2.0 * s,
                            x + 6.0 * s,
                            y - 9.0 * s,
                            1.4 * s,
                        );
                    }
                }
                _ => {
                    let (x0, x1) = (run[0].0, run[run.len() - 1].0);
                    out.rect(
                        x0 - 0.5 * s,
                        bot - beam_h,
                        x1 - x0 + 1.0 * s,
                        beam_h,
                        Ink::Primary,
                    );
                    let max_l = run.iter().map(|e| e.1).max().unwrap_or(1);
                    for l in 1..max_l {
                        let y = bot - beam_h - l as f32 * beam_gap;
                        for (n, e) in run.iter().enumerate() {
                            if e.1 <= l {
                                continue;
                            }
                            let prev = n.checked_sub(1).map(|p| run[p]);
                            let next = run.get(n + 1).copied();
                            if let Some(nx) = next.filter(|nx| nx.1 > l) {
                                out.rect(
                                    e.0 - 0.5 * s,
                                    y,
                                    nx.0 - e.0 + 1.0 * s,
                                    beam_h,
                                    Ink::Primary,
                                );
                            } else if prev.is_none_or(|p| p.1 <= l) {
                                // Lone sub-beat: a stub toward its neighbour.
                                let stub = 6.0 * s;
                                let sx = if next.is_some() || prev.is_none() {
                                    e.0
                                } else {
                                    e.0 - stub
                                };
                                out.rect(sx, y, stub, beam_h, Ink::Primary);
                            }
                        }
                    }
                }
            }
            run.clear();
        };

        for beat in &voice.beats {
            let onset = t;
            let x = x_at(bb, onset);
            t += beat.duration.ticks();
            let d = beat.duration;
            let levels = if d.value >= 8 {
                d.value.trailing_zeros().saturating_sub(2)
            } else {
                0
            };
            let key = onset / group;
            if beat.is_rest() {
                flush_run(self.out, &mut run);
                rest(self.out, s, x, mid, d.value, d.dots);
                continue;
            }
            // Stem.
            match d.value {
                0 | 1 => {}
                2 => self.out.line(x, mid, x, bot, 1.0 * s, Ink::Primary),
                _ => self.out.line(x, top, x, bot, 1.0 * s, Ink::Primary),
            }
            // Dots after the stem, clear of any beams.
            let dot_y = if levels > 0 {
                bot - beam_h - 3.0 * s
            } else {
                bot - 2.0 * s
            };
            for n in 0..d.dots {
                self.out.dot(
                    x + (3.5 + 3.5 * f32::from(n)) * s,
                    dot_y,
                    1.3 * s,
                    Ink::Primary,
                );
            }
            if levels == 0 || beat.effects.grace {
                flush_run(self.out, &mut run);
                if levels > 0 {
                    run.push((x, levels, key));
                    flush_run(self.out, &mut run);
                }
                continue;
            }
            if run.last().is_some_and(|e| e.2 != key) {
                flush_run(self.out, &mut run);
            }
            run.push((x, levels, key));
        }
        flush_run(self.out, &mut run);

        // Tuplet numbers.
        let mut t = 0;
        let mut grp: Option<(u8, f32, f32, u32, u32)> = None; // (n, x0, x1, ticks, target)
        for beat in &voice.beats {
            let x = x_at(bb, t);
            t += beat.duration.ticks();
            let d = beat.duration;
            match d.tuplet {
                Some((n, mm)) if n > 1 => {
                    let g = grp.get_or_insert_with(|| {
                        let unit = 4 * TICKS_PER_QUARTER / u32::from(d.value.max(1));
                        (n, x, x, 0, unit * u32::from(mm))
                    });
                    g.2 = x;
                    g.3 += d.ticks();
                    if g.3 >= g.4 || g.0 != n {
                        let (x0, x1) = (g.1, g.2);
                        let mx = (x0 + x1) / 2.0;
                        let ty = bot + 10.0 * s;
                        // A bracket under the group, broken for the number.
                        if x1 - x0 > 16.0 * s {
                            let by = ty;
                            let gap = 5.0 * s;
                            let hook = 3.0 * s;
                            let ink = Ink::Secondary;
                            self.out.line(x0, by - hook, x0, by, 0.8 * s, ink);
                            self.out.line(x0, by, mx - gap, by, 0.8 * s, ink);
                            self.out.line(mx + gap, by, x1, by, 0.8 * s, ink);
                            self.out.line(x1, by, x1, by - hook, 0.8 * s, ink);
                        }
                        self.out.text(
                            mx,
                            ty,
                            g.0.to_string(),
                            9.0 * s,
                            ITALIC,
                            TextAnchor::Center,
                            Ink::Primary,
                        );
                        grp = None;
                    }
                }
                _ => grp = None,
            }
        }
    }
}

/// A rest symbol drawn with simple shapes, centred on (x, y).
fn rest(out: &mut Out, s: f32, x: f32, y: f32, value: u8, dots: u8) {
    let w = 1.6 * s;
    match value {
        // Whole rest: a block hanging from a short line; half rest: a block sitting on one.
        0 | 1 => {
            let ly = y - 3.0 * s;
            out.line(x - 6.5 * s, ly, x + 6.5 * s, ly, 1.0 * s, Ink::Primary);
            out.rect(x - 4.5 * s, ly, 9.0 * s, 3.5 * s, Ink::Primary);
        }
        2 => {
            let ly = y + 1.0 * s;
            out.line(x - 6.5 * s, ly, x + 6.5 * s, ly, 1.0 * s, Ink::Primary);
            out.rect(x - 4.5 * s, ly - 3.5 * s, 9.0 * s, 3.5 * s, Ink::Primary);
        }
        4 => {
            let pts = [
                (-1.5, -8.0),
                (2.0, -3.5),
                (-1.5, 0.5),
                (2.0, 4.5),
                (-1.0, 4.0),
                (1.0, 8.0),
            ];
            for p in pts.windows(2) {
                out.line(
                    x + p[0].0 * s,
                    y + p[0].1 * s,
                    x + p[1].0 * s,
                    y + p[1].1 * s,
                    w,
                    Ink::Primary,
                );
            }
        }
        _ => {
            let levels = value.trailing_zeros().saturating_sub(2).max(1);
            let h = 4.0 * s;
            let top = y - 5.0 * s;
            for l in 0..levels {
                let ly = top + l as f32 * h;
                out.dot(x - 1.5 * s, ly, 1.6 * s, Ink::Primary);
                out.line(
                    x - 1.5 * s,
                    ly + 1.0 * s,
                    x + 2.5 * s,
                    ly - 0.5 * s,
                    1.0 * s,
                    Ink::Primary,
                );
            }
            out.line(
                x + 2.5 * s,
                top - 0.5 * s,
                x - 0.5 * s,
                top + (levels as f32) * h + 4.0 * s,
                1.1 * s,
                Ink::Primary,
            );
        }
    }
    for n in 0..dots {
        out.dot(
            x + (6.0 + 3.5 * f32::from(n)) * s,
            y - 2.0 * s,
            1.3 * s,
            Ink::Primary,
        );
    }
}

/// Staff position of a General MIDI drum note, in half line-spaces below the top line
/// (negative = above the staff), and whether it's drawn as a cymbal "x".
fn drum_position(key: u8) -> (i8, bool) {
    match key {
        35 | 36 => (7, false),           // kick: bottom space
        37..=40 => (3, false),           // snare, side stick, clap
        41 => (6, false),                // low floor tom
        43 => (5, false),                // floor tom
        45 | 47 => (2, false),           // mid toms
        48 | 50 => (1, false),           // high toms
        42 | 46 => (-1, true),           // hi-hat closed / open: above the staff
        44 => (9, true),                 // pedal hi-hat: below the staff
        49 | 52 | 55 | 57 => (-2, true), // crash, china, splash: ledger line
        51 | 59 => (0, true),            // ride: top line
        53 => (0, false),                // ride bell
        _ => (4, false),
    }
}

/// The last beat of voice `v` in a laid-out bar: its x, the beat, and the voice's beat count.
fn last_beat<'b>(ctx: &'b Ctx, bb: &BarBox, v: usize) -> Option<(f32, &'b Beat, usize)> {
    let beats = &ctx.bar(bb.master_bar as usize).voices.get(v)?.beats;
    let last = beats.last()?;
    let total: u32 = beats.iter().map(|b| b.duration.ticks()).sum();
    Some((x_at(bb, total - last.duration.ticks()), last, beats.len()))
}

/// x of the column for `tick` in a laid-out bar (nearest column if there's no exact onset).
fn x_at(bb: &BarBox, tick: u32) -> f32 {
    let k = bb.beats.partition_point(|b| b.tick_in_bar < tick);
    match (
        bb.beats.get(k),
        k.checked_sub(1).and_then(|p| bb.beats.get(p)),
    ) {
        (Some(b), _) if b.tick_in_bar == tick => b.x,
        (Some(b), None) => b.x,
        (_, Some(p)) => p.x,
        (None, None) => bb.frame.x,
    }
}

fn label_len(note: &Note) -> usize {
    if note.effects.dead {
        return 1;
    }
    let d = digits(note.fret);
    if note.effects.harmonic.is_some() || note.effects.ghost || note.tie_destination {
        d + 2
    } else {
        d
    }
}

fn note_label(note: &Note) -> String {
    let e = &note.effects;
    if e.dead {
        "x".to_string()
    } else if e.harmonic.is_some() {
        format!("<{}>", note.fret)
    } else if e.ghost || note.tie_destination {
        format!("({})", note.fret)
    } else {
        note.fret.to_string()
    }
}

fn bend_label(semitones: f32) -> String {
    // In quarter steps: 1 semitone = ½ step, 2 semitones = a full step.
    let quarters = (semitones * 2.0).round() as i32;
    match quarters {
        ..=0 => "0".to_string(),
        1 => "¼".to_string(),
        2 => "½".to_string(),
        4 => "full".to_string(),
        q if q % 4 == 0 => format!("{}", q / 4),
        q if q % 4 == 2 => format!("{}½", q / 4),
        q => format!("{:.2}", q as f32 / 4.0),
    }
}

/// Text for a navigation direction, and whether it belongs at the end of the bar (jumps).
fn direction_label(d: Direction) -> (&'static str, bool) {
    use Direction::*;
    match d {
        Coda => ("Coda", false),
        DoubleCoda => ("Double Coda", false),
        Segno => ("Segno", false),
        SegnoSegno => ("Segno Segno", false),
        Fine => ("Fine", true),
        DaCapo => ("D.C.", true),
        DaCapoAlCoda => ("D.C. al Coda", true),
        DaCapoAlDoubleCoda => ("D.C. al Double Coda", true),
        DaCapoAlFine => ("D.C. al Fine", true),
        DalSegno => ("D.S.", true),
        DalSegnoAlCoda => ("D.S. al Coda", true),
        DalSegnoAlDoubleCoda => ("D.S. al Double Coda", true),
        DalSegnoAlFine => ("D.S. al Fine", true),
        DalSegnoSegno => ("D.S.S.", true),
        DalSegnoSegnoAlCoda => ("D.S.S. al Coda", true),
        DalSegnoSegnoAlDoubleCoda => ("D.S.S. al Double Coda", true),
        DalSegnoSegnoAlFine => ("D.S.S. al Fine", true),
        DaCoda => ("To Coda", true),
        DaDoubleCoda => ("To Double Coda", true),
    }
}
