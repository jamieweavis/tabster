use super::test_scores::ScoreBuilder;
use super::*;
use crate::model::Direction::*;

fn order(builder: ScoreBuilder) -> Vec<u32> {
    Timeline::new(&builder.build())
        .played_bars()
        .iter()
        .map(|b| b.master_bar)
        .collect()
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

#[test]
fn empty_score() {
    let t = Timeline::new(&Score::default());
    assert!(t.played_bars().is_empty());
    assert_eq!(t.total_ticks(), 0);
    assert_eq!(t.duration_secs(), 0.0);
    assert_eq!(t.tick_to_secs(100), 0.0);
    assert_eq!(t.secs_to_tick(1.0), 0);
    assert_eq!(t.position_at_secs(1.0), None);
    assert_eq!(t.played_index(0, 0), None);
}

#[test]
fn linear() {
    let t = Timeline::new(&ScoreBuilder::new(3).build());
    let bars = t.played_bars();
    assert_eq!(bars.len(), 3);
    for (i, b) in bars.iter().enumerate() {
        assert_eq!(b.master_bar, i as u32);
        assert_eq!(b.occurrence, 0);
        assert_eq!(b.start_tick, i as u64 * 3840);
        assert_eq!(b.end_tick, (i as u64 + 1) * 3840);
        assert!(close(b.start_secs, i as f64 * 2.0));
        assert!(close(b.end_secs, (i + 1) as f64 * 2.0));
    }
    assert!(close(t.duration_secs(), 6.0));
    assert_eq!(t.total_ticks(), 3 * 3840);
}

#[test]
fn simple_repeat() {
    assert_eq!(
        order(ScoreBuilder::new(4).repeat(1, 2, 2)),
        [0, 1, 2, 1, 2, 3]
    );
}

#[test]
fn repeat_three_times() {
    assert_eq!(
        order(ScoreBuilder::new(3).repeat(0, 1, 3)),
        [0, 1, 0, 1, 0, 1, 2]
    );
}

#[test]
fn occurrences_and_played_index() {
    let t = Timeline::new(&ScoreBuilder::new(4).repeat(1, 2, 2).build());
    let occ: Vec<u32> = t.played_bars().iter().map(|b| b.occurrence).collect();
    assert_eq!(occ, [0, 0, 0, 1, 1, 0]);
    assert_eq!(t.played_index(1, 0), Some(1));
    assert_eq!(t.played_index(1, 1), Some(3));
    assert_eq!(t.played_index(1, 2), None);
    assert_eq!(t.played_index(3, 0), Some(5));
    assert_eq!(t.played_index(9, 0), None);
}

#[test]
fn alternate_endings_1_2() {
    // |: A B [1. C :|] [2. D | E
    let b = ScoreBuilder::new(5)
        .repeat(0, 2, 2)
        .ending(2, 0b01)
        .ending(3, 0b10);
    assert_eq!(order(b), [0, 1, 2, 0, 1, 3, 4]);
}

#[test]
fn alternate_endings_12_3() {
    // |: A [1.2. B :|x3] [3. C | D
    let b = ScoreBuilder::new(4)
        .repeat(0, 1, 3)
        .ending(1, 0b011)
        .ending(2, 0b100);
    assert_eq!(order(b), [0, 1, 0, 1, 0, 2, 3]);
}

#[test]
fn three_endings_each_closing() {
    // |: A [1. B :|] [2. C :|] [3. D | E, both closes counting 3 passes.
    let b = ScoreBuilder::new(5)
        .repeat_start(0)
        .repeat_end(1, 3)
        .repeat_end(2, 3)
        .ending(1, 0b001)
        .ending(2, 0b010)
        .ending(3, 0b100);
    assert_eq!(order(b), [0, 1, 0, 2, 0, 3, 4]);
}

#[test]
fn endings_without_repeat_start() {
    // A B [1. C :|] [2. D  — repeat from the start.
    let b = ScoreBuilder::new(4)
        .repeat_end(2, 2)
        .ending(2, 0b01)
        .ending(3, 0b10);
    assert_eq!(order(b), [0, 1, 2, 0, 1, 3]);
}

#[test]
fn consecutive_repeats_with_endings() {
    // |: A [1. B :|] [2. C | D :| — second repeat has no start: from after the endings.
    let b = ScoreBuilder::new(5)
        .repeat(0, 1, 2)
        .ending(1, 0b01)
        .ending(2, 0b10)
        .repeat_end(3, 2);
    assert_eq!(order(b), [0, 1, 0, 2, 3, 3, 4]);
}

#[test]
fn nested_repeats() {
    // |: A |: B :|x2 C :|x2 D
    let b = ScoreBuilder::new(4)
        .repeat_start(0)
        .repeat(1, 1, 2)
        .repeat_end(2, 2);
    assert_eq!(order(b), [0, 1, 1, 2, 0, 1, 1, 2, 3]);
}

#[test]
fn repeat_end_without_start_after_repeat() {
    // |: A :| B C :|  — the second repeat starts after the first one.
    let b = ScoreBuilder::new(3).repeat(0, 0, 2).repeat_end(2, 2);
    assert_eq!(order(b), [0, 0, 1, 2, 1, 2]);
}

#[test]
fn dal_segno_al_coda() {
    // A B(segno) C(to coda) D(D.S. al coda) E(coda) F
    let b = ScoreBuilder::new(6)
        .direction(1, Segno)
        .direction(2, DaCoda)
        .direction(3, DalSegnoAlCoda)
        .direction(4, Coda);
    assert_eq!(order(b), [0, 1, 2, 3, 1, 2, 4, 5]);
}

#[test]
fn da_capo_al_fine() {
    // A B(fine) C(D.C. al fine)
    let b = ScoreBuilder::new(3)
        .direction(1, Fine)
        .direction(2, DaCapoAlFine);
    assert_eq!(order(b), [0, 1, 2, 0, 1]);
}

#[test]
fn da_capo_plain_plays_to_end() {
    let b = ScoreBuilder::new(3).direction(1, Fine).direction(2, DaCapo);
    assert_eq!(order(b), [0, 1, 2, 0, 1, 2]);
}

#[test]
fn double_coda() {
    // A(segno segno) B(to double coda) C(D.S.S. al double coda) D(double coda)
    let b = ScoreBuilder::new(4)
        .direction(0, SegnoSegno)
        .direction(1, DaDoubleCoda)
        .direction(2, DalSegnoSegnoAlDoubleCoda)
        .direction(3, DoubleCoda);
    assert_eq!(order(b), [0, 1, 2, 0, 1, 3]);
}

#[test]
fn repeat_inside_dal_segno_played_once() {
    // A(segno) |: B :| C(D.S. al fine) ... with fine on C: after the jump B plays once.
    let b = ScoreBuilder::new(4)
        .direction(0, Segno)
        .repeat(1, 1, 2)
        .direction(2, Fine)
        .direction(2, DalSegnoAlFine);
    // First pass: A B B C, jump, A B C (Fine).
    assert_eq!(order(b), [0, 1, 1, 2, 0, 1, 2]);
}

#[test]
fn endings_after_jump_take_last_ending() {
    // |: A [1. B :|] [2. C | D(D.C.)  → A B A C D A C D
    let b = ScoreBuilder::new(4)
        .repeat(0, 1, 2)
        .ending(1, 0b01)
        .ending(2, 0b10)
        .direction(3, DaCapo);
    assert_eq!(order(b), [0, 1, 0, 2, 3, 0, 2, 3]);
}

#[test]
fn jump_after_repeat_finishes() {
    // A(segno) |: B C(D.S.) :|  → repeat first, then the jump.
    let b = ScoreBuilder::new(4)
        .direction(0, Segno)
        .repeat(1, 2, 2)
        .direction(2, DalSegno);
    assert_eq!(order(b), [0, 1, 2, 1, 2, 0, 1, 2, 3]);
}

#[test]
fn malformed_terminates() {
    // D.S. without segno and To Coda without coda are ignored.
    let b = ScoreBuilder::new(3)
        .direction(1, DalSegnoAlCoda)
        .direction(2, DaCoda);
    assert_eq!(order(b), [0, 1, 2]);

    // repeat_end without start repeats from the beginning.
    assert_eq!(
        order(ScoreBuilder::new(3).repeat_end(1, 2)),
        [0, 1, 0, 1, 2]
    );

    // A jump loop: D.C. at every bar. Each jump fires once, so it terminates.
    let mut b = ScoreBuilder::new(5);
    for i in 0..5 {
        b = b
            .direction(i, DaCapo)
            .direction(i, Segno)
            .repeat_end(i, 255);
    }
    let t = Timeline::new(&b.build());
    assert!(t.played_bars().len() <= 5 * 10 + 1000);

    // Huge repeat counts and every-bar endings.
    let mut b = ScoreBuilder::new(20);
    for i in 0..20 {
        b = b.repeat_start(i).repeat_end(i, 255).ending(i, 0xff);
    }
    let t = Timeline::new(&b.build());
    assert!(!t.played_bars().is_empty());
    assert!(t.played_bars().len() <= 20 * 10 + 1000);

    // All bars in endings that never match: nothing plays.
    let t = Timeline::new(
        &ScoreBuilder::new(2)
            .ending(0, 0b10)
            .ending(1, 0b100)
            .build(),
    );
    assert!(t.played_bars().is_empty());
    assert_eq!(t.position_at_secs(0.0), None);
}

#[test]
fn tempo_change_at_bar_start() {
    // 120 bpm for bar 0 (2 s), then 60 bpm (4 s per bar).
    let t = Timeline::new(&ScoreBuilder::new(3).tempo_change(1, 0.0, 60.0).build());
    let bars = t.played_bars();
    assert!(close(bars[1].start_secs, 2.0));
    assert!(close(bars[1].end_secs, 6.0));
    assert!(close(bars[2].end_secs, 10.0));
    assert!(close(t.tick_to_secs(3840 + 960), 3.0));
}

#[test]
fn tempo_change_at_start_overrides_initial() {
    let t = Timeline::new(
        &ScoreBuilder::new(1)
            .tempo(90.0)
            .tempo_change(0, 0.0, 60.0)
            .build(),
    );
    assert!(close(t.duration_secs(), 4.0));
}

#[test]
fn tempo_change_mid_bar() {
    // Halfway through bar 0, 120 → 60: 1 s + 2 s.
    let t = Timeline::new(&ScoreBuilder::new(2).tempo_change(0, 0.5, 60.0).build());
    let bars = t.played_bars();
    assert!(close(bars[0].end_secs, 3.0));
    assert!(close(bars[1].end_secs, 7.0));
    assert!(close(t.tick_to_secs(1920), 1.0));
    assert!(close(t.tick_to_secs(2880), 2.0));
    assert_eq!(t.secs_to_tick(2.0), 2880);
    let p = t.position_at_secs(2.0).unwrap();
    assert_eq!((p.played_bar, p.tick_in_bar), (0, 2880));
    assert!(close(p.fraction, 0.75));
}

#[test]
fn tempo_changes_reapply_on_repeat() {
    // |: A B(→60 at start) :| — the second pass of A runs at 60 (tempo carries over), and
    // B's change applies again.
    let t = Timeline::new(
        &ScoreBuilder::new(2)
            .repeat(0, 1, 2)
            .tempo_change(1, 0.0, 60.0)
            .build(),
    );
    let ends: Vec<f64> = t.played_bars().iter().map(|b| b.end_secs).collect();
    assert!(close(ends[0], 2.0) && close(ends[1], 6.0));
    assert!(close(ends[2], 10.0) && close(ends[3], 14.0));
}

#[test]
fn invalid_tempos_ignored() {
    let t = Timeline::new(
        &ScoreBuilder::new(2)
            .tempo(0.0)
            .tempo_change(1, 0.0, -5.0)
            .tempo_change(1, 0.5, f64::NAN)
            .tempo_change(7, 0.0, 60.0)
            .build(),
    );
    assert!(close(t.duration_secs(), 4.0));
}

#[test]
fn tick_secs_round_trip() {
    let t = Timeline::new(
        &ScoreBuilder::new(8)
            .tempo(97.0)
            .repeat(2, 4, 2)
            .tempo_change(1, 0.3, 141.5)
            .tempo_change(3, 0.0, 61.0)
            .tempo_change(5, 0.9, 200.0)
            .build(),
    );
    let total = t.total_ticks();
    let mut prev = -1.0;
    for tick in (0..=total).step_by(7) {
        let secs = t.tick_to_secs(tick);
        assert!(secs > prev);
        prev = secs;
        assert_eq!(t.secs_to_tick(secs), tick);
    }
    assert_eq!(t.secs_to_tick(t.duration_secs()), total);
    assert!(close(t.tick_to_secs(total), t.duration_secs()));
}

#[test]
fn clamping() {
    let t = Timeline::new(&ScoreBuilder::new(2).build());
    assert_eq!(t.secs_to_tick(-5.0), 0);
    assert_eq!(t.secs_to_tick(f64::NAN), 0);
    assert_eq!(t.secs_to_tick(100.0), 7680);
    assert!(close(t.tick_to_secs(1_000_000), 4.0));
    let p = t.position_at_secs(-1.0).unwrap();
    assert_eq!((p.played_bar, p.tick_in_bar), (0, 0));
    let p = t.position_at_secs(100.0).unwrap();
    assert_eq!((p.played_bar, p.master_bar, p.tick_in_bar), (1, 1, 3840));
    assert!(close(p.fraction, 1.0));
}

#[test]
fn position_in_repeat() {
    let t = Timeline::new(&ScoreBuilder::new(3).repeat(0, 1, 2).build());
    // Played: 0 1 0 1 2; 5 s is halfway through the third played bar (master 0, second pass).
    let p = t.position_at_secs(5.0).unwrap();
    assert_eq!((p.played_bar, p.master_bar, p.tick_in_bar), (2, 0, 1920));
    assert!(close(p.fraction, 0.5));
    // Exactly at a bar boundary, the position is the start of the next bar.
    let p = t.position_at_secs(4.0).unwrap();
    assert_eq!((p.played_bar, p.tick_in_bar), (2, 0));
}

#[test]
fn pickup_bar() {
    let t = Timeline::new(&ScoreBuilder::new(3).pickup(0, 1).build());
    let bars = t.played_bars();
    assert_eq!(bars[0].end_tick, 960);
    assert!(close(bars[0].end_secs, 0.5));
    assert_eq!(bars[1].start_tick, 960);
    assert!(close(bars[2].end_secs, 4.5));
}

#[test]
fn empty_bar_uses_time_signature() {
    let t = Timeline::new(
        &ScoreBuilder::new(2)
            .time_signature(0, 6, 8)
            .empty(0)
            .build(),
    );
    assert_eq!(t.played_bars()[0].end_tick, 2880);
    assert!(close(t.played_bars()[0].end_secs, 1.5));
}
