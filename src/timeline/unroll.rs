//! Unrolling repeats, alternate endings and directions into the order bars are played in.
//!
//! Semantics (modelled on Guitar Pro / alphaTab playback):
//! - **Repeats.** `repeat_start` opens a group; a bar with `repeat_end = n` jumps back to the
//!   group's start until the section has been played `n` times. A `repeat_end` with no open
//!   group repeats from the *implicit start*: bar 0, the bar after the last finished repeat (or
//!   alternate-ending run), or the target of the last D.C./D.S. jump. Groups nest (a stack).
//! - **Alternate endings.** On pass `k` (1-based) of the innermost group, a bar whose mask is
//!   non-zero plays only if bit `k - 1` is set; skipped bars are skipped entirely (their
//!   `repeat_end` and directions are ignored). Bars with mask 0 always play. Leaving a run of
//!   ending bars (reaching the next mask-0 bar in sequence) closes the group they belonged to.
//! - **Directions.** D.C./D.S. jumps (and the "To Coda" `DaCoda`/`DaDoubleCoda`) execute at the
//!   end of the bar carrying them, at most once per bar, and only after that bar's repeat has
//!   finished. A jump with no target (e.g. D.S. without a Segno) is ignored. After a D.C./D.S.
//!   jump, repeats are not taken (each section plays once) and of each alternate-ending run only
//!   the bars carrying the run's highest ending number play (the "last time" ending).
//!   "al Coda" / "al Double Coda" arms the matching To Coda jump; "al Fine" stops playback at
//!   the end of the first `Fine` bar played afterwards. To Coda is ignored unless armed.
//! - Unrolling stops after `10 × bars + 1000` steps, so malformed input always terminates.

use crate::model::{Direction, MasterBar};

#[derive(Debug, Clone, Copy)]
struct Group {
    start: usize,
    /// 1-based pass number.
    pass: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Start,
    Direction(Direction),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Then {
    Nothing,
    /// Arm the To Coda jump of this kind (`DaCoda` or `DaDoubleCoda`).
    ToCoda(Direction),
    Fine,
}

fn da_jump(d: Direction) -> Option<(Target, Then)> {
    use Direction::*;
    let segno = Target::Direction(Segno);
    let segno2 = Target::Direction(SegnoSegno);
    Some(match d {
        DaCapo => (Target::Start, Then::Nothing),
        DaCapoAlCoda => (Target::Start, Then::ToCoda(DaCoda)),
        DaCapoAlDoubleCoda => (Target::Start, Then::ToCoda(DaDoubleCoda)),
        DaCapoAlFine => (Target::Start, Then::Fine),
        DalSegno => (segno, Then::Nothing),
        DalSegnoAlCoda => (segno, Then::ToCoda(DaCoda)),
        DalSegnoAlDoubleCoda => (segno, Then::ToCoda(DaDoubleCoda)),
        DalSegnoAlFine => (segno, Then::Fine),
        DalSegnoSegno => (segno2, Then::Nothing),
        DalSegnoSegnoAlCoda => (segno2, Then::ToCoda(DaCoda)),
        DalSegnoSegnoAlDoubleCoda => (segno2, Then::ToCoda(DaDoubleCoda)),
        DalSegnoSegnoAlFine => (segno2, Then::Fine),
        _ => return None,
    })
}

/// Master bar indices in played order.
pub fn unroll(bars: &[MasterBar]) -> Vec<u32> {
    let n = bars.len();
    let mut out = Vec::with_capacity(n);
    if n == 0 {
        return out;
    }
    let cap = n * 10 + 1000;
    let find = |d: Direction| bars.iter().position(|b| b.directions.contains(&d));

    // For each bar in a run of consecutive alternate-ending bars, the bit of the run's highest
    // ending number.
    let mut last_ending = vec![0u8; n];
    let mut run_start = 0;
    for i in 0..=n {
        if i == n || bars[i].alternate_endings == 0 {
            let all: u8 = bars[run_start..i]
                .iter()
                .fold(0, |acc, b| acc | b.alternate_endings);
            let top = if all == 0 {
                0
            } else {
                1 << (7 - all.leading_zeros())
            };
            last_ending[run_start..i].fill(top);
            run_start = i + 1;
        }
    }

    let mut executed = vec![false; n];
    let mut stack: Vec<Group> = Vec::new();
    // Pass of a group that just finished at its repeat_end, for ending bars right after it.
    let mut lingering: Option<u32> = None;
    // Stack depth of the group owning the current alternate-ending run.
    let mut ending_owner: Option<usize> = None;
    let mut implicit_start = 0usize;
    let mut after_jump = false;
    let mut to_coda: Option<Direction> = None;
    let mut fine = false;
    let mut came_back = false;

    let mut i = 0usize;
    let mut steps = 0usize;
    while i < n && steps < cap {
        steps += 1;
        let bar = &bars[i];
        let mask = bar.alternate_endings;

        if mask == 0 {
            lingering = None;
            if let Some(depth) = ending_owner.take() {
                if depth == stack.len() && depth > 0 {
                    stack.pop();
                }
                implicit_start = i;
            }
        }
        if bar.repeat_start && !came_back && !after_jump {
            stack.push(Group { start: i, pass: 1 });
            lingering = None;
        }
        came_back = false;

        let play = if mask == 0 {
            true
        } else if after_jump {
            mask & last_ending[i] != 0
        } else {
            let pass = lingering.or(stack.last().map(|g| g.pass)).unwrap_or(1);
            if lingering.is_none() && !stack.is_empty() {
                ending_owner = Some(stack.len());
            }
            pass <= 8 && mask & (1 << (pass - 1)) != 0
        };
        if !play {
            i += 1;
            continue;
        }
        out.push(i as u32);

        if bar.repeat_end > 0 && !after_jump {
            let count = u32::from(bar.repeat_end);
            if stack.is_empty() {
                stack.push(Group {
                    start: implicit_start,
                    pass: 1,
                });
            }
            let depth = stack.len();
            let group = stack.last_mut().expect("just ensured non-empty");
            if group.pass < count {
                group.pass += 1;
                i = group.start;
                came_back = true;
                ending_owner = None;
                continue;
            }
            lingering = Some(group.pass);
            stack.pop();
            if ending_owner == Some(depth) {
                ending_owner = None;
            }
            implicit_start = i + 1;
        }

        if fine && bar.directions.contains(&Direction::Fine) {
            break;
        }

        // An armed To Coda fires once (it disarms itself); D.C./D.S. jumps fire once per bar.
        let mut jump = to_coda.filter(|d| bar.directions.contains(d)).map(|d| {
            let target = if d == Direction::DaCoda {
                Direction::Coda
            } else {
                Direction::DoubleCoda
            };
            (find(target), Then::Nothing)
        });
        if jump.is_none() && !executed[i] {
            jump = bar.directions.iter().find_map(|&d| {
                let (target, then) = da_jump(d)?;
                let index = match target {
                    Target::Start => Some(0),
                    Target::Direction(t) => find(t),
                };
                Some((index, then))
            });
            if jump.is_some() {
                executed[i] = true;
            }
        }
        if let Some((Some(target), then)) = jump {
            after_jump = true;
            stack.clear();
            lingering = None;
            ending_owner = None;
            implicit_start = target;
            match then {
                Then::Nothing => to_coda = None,
                Then::ToCoda(d) => to_coda = Some(d),
                Then::Fine => fine = true,
            }
            i = target;
            continue;
        }
        i += 1;
    }
    out
}
