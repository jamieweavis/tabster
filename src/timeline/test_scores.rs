//! Small builder for hand-made test scores (one track, quarter-note beats).

use crate::model::{
    Bar, Beat, Direction, Duration, MasterBar, Score, TempoChange, TimeSignature, Track, Voice,
};

pub(crate) struct ScoreBuilder {
    score: Score,
}

fn bar_of(time_signature: TimeSignature, beats: usize) -> Bar {
    let beat = Beat {
        duration: Duration {
            value: time_signature.denominator,
            dots: 0,
            tuplet: None,
        },
        ..Default::default()
    };
    Bar {
        voices: vec![Voice {
            beats: vec![beat; beats],
        }],
    }
}

impl ScoreBuilder {
    /// `bars` full bars of 4/4 at 120 bpm (2 seconds each).
    pub(crate) fn new(bars: usize) -> Self {
        let ts = TimeSignature::default();
        Self {
            score: Score {
                title: "Test".into(),
                initial_tempo: 120.0,
                tracks: vec![Track {
                    name: "Guitar".into(),
                    tuning: vec![64, 59, 55, 50, 45, 40],
                    bars: vec![bar_of(ts, 4); bars],
                    ..Default::default()
                }],
                master_bars: vec![MasterBar::default(); bars],
                ..Default::default()
            },
        }
    }

    pub(crate) fn tempo(mut self, bpm: f64) -> Self {
        self.score.initial_tempo = bpm;
        self
    }

    /// `|:` on `start`, `:|` played `count` times total on `end`.
    pub(crate) fn repeat(self, start: usize, end: usize, count: u8) -> Self {
        self.repeat_start(start).repeat_end(end, count)
    }

    pub(crate) fn repeat_start(mut self, bar: usize) -> Self {
        self.score.master_bars[bar].repeat_start = true;
        self
    }

    pub(crate) fn repeat_end(mut self, bar: usize, count: u8) -> Self {
        self.score.master_bars[bar].repeat_end = count;
        self
    }

    pub(crate) fn ending(mut self, bar: usize, mask: u8) -> Self {
        self.score.master_bars[bar].alternate_endings = mask;
        self
    }

    pub(crate) fn direction(mut self, bar: usize, direction: Direction) -> Self {
        self.score.master_bars[bar].directions.push(direction);
        self
    }

    pub(crate) fn tempo_change(mut self, bar: u32, position: f64, bpm: f64) -> Self {
        self.score
            .tempo_changes
            .push(TempoChange { bar, position, bpm });
        self
    }

    /// Make `bar` a pickup of `beats` quarter notes.
    pub(crate) fn pickup(mut self, bar: usize, beats: usize) -> Self {
        let ts = self.score.master_bars[bar].time_signature;
        self.score.master_bars[bar].anacrusis = true;
        self.score.tracks[0].bars[bar] = bar_of(ts, beats);
        self
    }

    /// Give `bar` no beats at all (its length then comes from the time signature).
    pub(crate) fn empty(mut self, bar: usize) -> Self {
        self.score.tracks[0].bars[bar] = Bar::default();
        self
    }

    pub(crate) fn time_signature(mut self, bar: usize, numerator: u8, denominator: u8) -> Self {
        let ts = TimeSignature {
            numerator,
            denominator,
        };
        self.score.master_bars[bar].time_signature = ts;
        self.score.tracks[0].bars[bar] = bar_of(ts, usize::from(numerator));
        self
    }

    pub(crate) fn build(self) -> Score {
        self.score
    }
}
