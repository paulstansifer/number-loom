//! Working out the clues that couldn't be read (that are "blotted", usually because they've been
//! crossed out) from the grid. Someone who's crossed out a clue has usually finished its block,
//! so the grid says what it was.
//!
//! In each lane with blotted clues:
//!
//! - If every cell is decided, the clues are the lane's blocks (however many clues were read).
//! - Otherwise, from each end, each finished block is matched with the next clue, up to the
//!   first undecided cell. (A block that runs into undecided cells might not be finished, but if
//!   its clue is blotted, it probably is.) A clue that disagrees with its block is left as it is:
//!   the person might have made a mistake.
//! - Blotted clues left in the middle are matched, in order, with the finished blocks there whose
//!   lengths don't match any clue that could be read; or, failing that, with all the finished
//!   blocks; or with all the blocks; whichever there are the right number of (and leave room for
//!   the clues on either side).
//!
//! Clues the app has dimmed (as some do, for clues that are done) are worked out the same way,
//! since the grid likely has their blocks, but where the grid can't say, they stay as they read.
//!
//! Then, if only one blotted clue is left in the whole puzzle, the totals say what it is: the
//! rows and the columns fill the same number of cells. Any lanes that still have blotted clues go
//! back to what the first pass read, if it read anything.

use crate::cells::State;
use crate::clue_layout::{BLOTTED, clue_text};

pub struct Repaired {
    pub cols: Vec<Vec<u16>>,
    pub rows: Vec<Vec<u16>>,
    /// How many blotted clues were worked out.
    pub filled: usize,
    /// Each blotted clue that was worked out, by where it was in the clues as read: whether it's
    /// a column's, which lane, where in the lane, and what it is.
    pub labels: Vec<(bool, usize, usize, u16)>,
    /// What was done, lane by lane, for a human to check.
    pub notes: Vec<String>,
}

/// The blocks of filled cells in `cells[from..to]`, as `start..end` ranges.
fn blocks(cells: &[State], from: usize, to: usize) -> Vec<(usize, usize)> {
    let mut blocks = vec![];
    let mut i = from;
    while i < to {
        if cells[i] == State::Filled {
            let end = i + cells[i..to]
                .iter()
                .take_while(|&&c| c == State::Filled)
                .count();
            blocks.push((i, end));
            i = end;
        } else {
            i += 1;
        }
    }
    blocks
}

/// Whether nothing more can be added to the block: it's crossed out (or at the edge) on both
/// sides.
fn finished(cells: &[State], (start, end): (usize, usize)) -> bool {
    (start == 0 || cells[start - 1] == State::Crossed)
        && (end == cells.len() || cells[end] == State::Crossed)
}

/// Match blocks with clues from the start of the lane, up to the first undecided cell, filling in
/// blotted clues. Returns where the cells not accounted for start, and how many clues were used;
/// or `None` if there are more blocks than clues.
fn walk(clues: &mut [u16], cells: &[State]) -> Option<(usize, usize)> {
    let (mut i, mut k) = (0, 0);
    loop {
        while cells.get(i) == Some(&State::Crossed) {
            i += 1;
        }
        if cells.get(i) != Some(&State::Filled) {
            return Some((i, k));
        }
        let end = i + cells[i..]
            .iter()
            .take_while(|&&c| c == State::Filled)
            .count();
        let clue = clues.get_mut(k)?;
        let length = (end - i) as u16;
        if cells.get(end) == Some(&State::Undecided) {
            // It may not be finished, but if its clue has been crossed out, it probably is.
            if *clue != BLOTTED {
                return Some((i, k));
            }
            *clue = length;
            return Some((end, k + 1));
        }
        if *clue == BLOTTED {
            *clue = length;
        }
        (i, k) = (end, k + 1);
    }
}

/// `clues`, with as many of its blotted clues worked out from `cells` as possible.
pub fn lane(clues: &[u16], cells: &[State]) -> Vec<u16> {
    if !clues.contains(&BLOTTED) || cells.is_empty() {
        return clues.to_vec();
    }
    if !cells.contains(&State::Undecided) {
        return blocks(cells, 0, cells.len())
            .into_iter()
            .map(|(start, end)| (end - start) as u16)
            .collect();
    }
    let mut repaired = clues.to_vec();
    let n = repaired.len();
    let Some((from, before)) = walk(&mut repaired, cells) else {
        return clues.to_vec();
    };
    // And from the other end, with the clues that are left.
    let mut backwards: Vec<u16> = repaired[before..].iter().rev().copied().collect();
    let reversed: Vec<State> = cells.iter().rev().copied().collect();
    let Some((from_end, after)) = walk(&mut backwards, &reversed) else {
        return clues.to_vec();
    };
    backwards.reverse();
    repaired[before..].copy_from_slice(&backwards);
    let to = cells.len() - from_end;

    let middle = &mut repaired[before..n - after];
    let blotted: Vec<usize> = (0..middle.len())
        .filter(|&i| middle[i] == BLOTTED)
        .collect();
    if blotted.is_empty() {
        return repaired;
    }
    let known: Vec<u16> = middle.iter().copied().filter(|&n| n != BLOTTED).collect();
    let all = blocks(cells, from, to);
    let done: Vec<(usize, usize)> = all
        .iter()
        .copied()
        .filter(|&b| finished(cells, b))
        .collect();
    let unknown: Vec<(usize, usize)> = done
        .iter()
        .copied()
        .filter(|&(start, end)| !known.contains(&((end - start) as u16)))
        .collect();
    // The clues before and after each one have to fit on either side of its block.
    let room = |clues: &[u16]| -> usize {
        clues
            .iter()
            .map(|&n| if n == BLOTTED { 2 } else { n as usize + 1 })
            .sum()
    };
    let fits = |chosen: &[(usize, usize)]| {
        blotted.iter().zip(chosen).all(|(&i, &(start, end))| {
            room(&middle[..i]) <= start - from && room(&middle[i + 1..]) <= to - end
        })
    };
    if let Some(chosen) = [unknown, done, all]
        .into_iter()
        .find(|candidates| candidates.len() == blotted.len() && fits(candidates))
    {
        for (&i, (start, end)) in blotted.iter().zip(chosen) {
            middle[i] = (end - start) as u16;
        }
    }
    repaired
}

/// `clues`, with the ones that are dimmed worked out from `cells` like blotted ones, where
/// possible, and otherwise left as they read.
fn lane_dimmed(clues: &[u16], dimmed: &[bool], cells: &[State]) -> Vec<u16> {
    let masked: Vec<u16> = clues
        .iter()
        .enumerate()
        .map(|(i, &n)| {
            if dimmed.get(i) == Some(&true) {
                BLOTTED
            } else {
                n
            }
        })
        .collect();
    let mut worked_out = lane(&masked, cells);
    // (A lane redone from the grid has nothing left blotted; otherwise, the clues line up.)
    if worked_out.len() == clues.len() {
        for (now, &read) in worked_out.iter_mut().zip(clues) {
            if *now == BLOTTED {
                *now = read;
            }
        }
    }
    worked_out
}

/// Lane `l`'s dimmed clues.
fn dims(dimmed: &[Vec<bool>], l: usize) -> &[bool] {
    dimmed.get(l).map_or(&[], |d| &d[..])
}

fn show(clues: &[u16]) -> String {
    if clues.is_empty() {
        return "(empty)".to_string();
    }
    let numbers: Vec<String> = clues.iter().map(|&n| clue_text(n)).collect();
    numbers.join(" ")
}

/// The clues, with the blotted and dimmed ones worked out from the grid (`states`,
/// `[row][column]`) where possible. `col_dimmed` and `row_dimmed` say which are dimmed, and
/// `col_backups` and `row_backups` are what to fall back on for lanes where blotted clues can't
/// be worked out (see `reread::Reread`).
pub fn repair(
    cols: &[Vec<u16>],
    rows: &[Vec<u16>],
    col_dimmed: &[Vec<bool>],
    row_dimmed: &[Vec<bool>],
    col_backups: &[Option<Vec<u16>>],
    row_backups: &[Option<Vec<u16>>],
    states: &[Vec<State>],
) -> Repaired {
    let (width, height) = (states.first().map_or(0, Vec::len), states.len());
    let column = |c: usize| -> Vec<State> {
        if c < width {
            states.iter().map(|row| row[c]).collect()
        } else {
            vec![] // (A lane past the end of the grid.)
        }
    };
    let row = |r: usize| -> Vec<State> { states.get(r).cloned().unwrap_or_default() };
    let mut repaired = Repaired {
        cols: cols
            .iter()
            .enumerate()
            .map(|(c, clues)| lane_dimmed(clues, dims(col_dimmed, c), &column(c)))
            .collect(),
        rows: rows
            .iter()
            .enumerate()
            .map(|(r, clues)| lane_dimmed(clues, dims(row_dimmed, r), &row(r)))
            .collect(),
        filled: 0,
        labels: vec![],
        notes: vec![],
    };

    let blotted = |lanes: &[Vec<u16>]| -> Vec<(usize, usize)> {
        lanes
            .iter()
            .enumerate()
            .flat_map(|(l, clues)| {
                (0..clues.len())
                    .filter(move |&i| clues[i] == BLOTTED)
                    .map(move |i| (l, i))
            })
            .collect()
    };
    let total = |lanes: &[Vec<u16>]| -> usize {
        lanes
            .iter()
            .flatten()
            .filter(|&&n| n != BLOTTED)
            .map(|&n| n as usize)
            .sum()
    };
    // If there's only one blotted clue left, it's whatever makes the totals agree (if that fits).
    let by_totals = |repaired: &mut Repaired| {
        let (in_cols, in_rows) = (blotted(&repaired.cols), blotted(&repaired.rows));
        let (col_total, row_total) = (total(&repaired.cols), total(&repaired.rows));
        let (lanes, (l, i), length, missing) = match (&in_cols[..], &in_rows[..]) {
            ([only], []) => (
                &mut repaired.cols,
                *only,
                height,
                row_total.checked_sub(col_total),
            ),
            ([], [only]) => (
                &mut repaired.rows,
                *only,
                width,
                col_total.checked_sub(row_total),
            ),
            _ => return,
        };
        let Some(missing) = missing.filter(|&m| m > 0) else {
            return;
        };
        let clues = &mut lanes[l];
        let others: usize = clues
            .iter()
            .filter(|&&n| n != BLOTTED)
            .map(|&n| n as usize)
            .sum();
        if others + missing + clues.len() - 1 <= length {
            clues[i] = missing as u16;
        }
    };
    by_totals(&mut repaired);
    for (lanes, backups) in [
        (&mut repaired.cols, col_backups),
        (&mut repaired.rows, row_backups),
    ] {
        for (clues, backup) in lanes.iter_mut().zip(backups) {
            if let Some(backup) = backup
                && clues.contains(&BLOTTED)
            {
                *clues = backup.clone();
            }
        }
    }
    by_totals(&mut repaired);

    // What changed.
    for (is_col, read, now) in [(true, cols, &repaired.cols), (false, rows, &repaired.rows)] {
        let what = if is_col { "column" } else { "row" };
        for (l, (read, now)) in read.iter().zip(now).enumerate() {
            if read == now {
                continue;
            }
            let worked_out = !now.contains(&BLOTTED);
            let backup = if is_col { col_backups } else { row_backups };
            let fell_back = worked_out && backup.get(l).is_some_and(|b| b.as_ref() == Some(now));
            if fell_back {
                repaired.notes.push(format!(
                    "{what} {}: couldn't work out {}, so went with the first reading, {}",
                    l + 1,
                    show(read),
                    show(now)
                ));
                continue;
            }
            repaired.notes.push(format!(
                "{what} {}: worked out {} as {} from the grid",
                l + 1,
                show(read),
                show(now)
            ));
            for i in (0..read.len()).filter(|&i| read[i] == BLOTTED) {
                if read.len() == now.len() && now[i] != BLOTTED {
                    repaired.labels.push((is_col, l, i, now[i]));
                }
            }
            let left = |c: &[u16]| c.iter().filter(|&&n| n == BLOTTED).count();
            repaired.filled += left(read) - left(now).min(left(read));
        }
    }
    repaired
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `#` filled in, `x` crossed out, `.` undecided.
    fn cells(text: &str) -> Vec<State> {
        text.chars()
            .map(|c| match c {
                '#' => State::Filled,
                'x' => State::Crossed,
                _ => State::Undecided,
            })
            .collect()
    }

    const B: u16 = BLOTTED;

    #[test]
    fn a_finished_lane_is_its_own_clue() {
        // However many clues were read.
        assert_eq!(lane(&[B], &cells("##x#xx###")), vec![2, 1, 3]);
        assert_eq!(lane(&[B, 4, 5, 6], &cells("x#xxxxxx")), vec![1]);
        // Without any blots, the clues stand, even if they disagree.
        assert_eq!(lane(&[1, 1], &cells("##x#xxxx")), vec![1, 1]);
    }

    #[test]
    fn from_the_ends() {
        assert_eq!(lane(&[B, 3, B], &cells("x##x.....x#")), vec![2, 3, 1]);
        // A clue that disagrees with its block is left as it is, but still uses it up.
        assert_eq!(lane(&[5, B, 3], &cells("##x#x....")), vec![5, 1, 3]);
        // A block running into undecided cells counts, if its clue is blotted...
        assert_eq!(lane(&[B, 3], &cells("x##......")), vec![2, 3]);
        // ...but if not, it might not be finished, so it's left for the middle. (Where it's the
        // only block, but the 3 has to come before the blotted clue's.)
        assert_eq!(lane(&[3, B], &cells("x##......")), vec![3, B]);
    }

    #[test]
    fn in_the_middle() {
        // A finished block of a length no clue has.
        assert_eq!(
            lane(&[1, 3, B, 1], &cells("..x...###.x##x...")),
            vec![1, 3, 2, 1]
        );
        assert_eq!(lane(&[1, B, 1], &cells("..x#x.x##x.")), vec![1, 2, 1]);
        // The finished blocks, if no lengths are unaccounted for.
        assert_eq!(lane(&[1, B, 1], &cells("..x#x.")), vec![1, 1, 1]);
        // All the blocks, if that's the only count that matches.
        assert_eq!(
            lane(&[1, B, B, 1], &cells("...##.x###x..")),
            vec![1, 2, 3, 1]
        );
        // Too many to choose from: give up.
        assert_eq!(lane(&[1, B, 1], &cells("..x##x.x##x.")), vec![1, B, 1]);
    }

    #[test]
    fn dimmed() {
        // Worked out like blotted clues...
        assert_eq!(
            lane_dimmed(&[7, 3], &[true, false], &cells("x#x....")),
            vec![1, 3]
        );
        // ...but where the grid can't say, as they read.
        assert_eq!(
            lane_dimmed(&[7, 3], &[true, false], &cells(".......")),
            vec![7, 3]
        );
        // A finished lane's clues are its blocks.
        assert_eq!(
            lane_dimmed(&[7, 3], &[true, false], &cells("x#x###x")),
            vec![1, 3]
        );
    }

    #[test]
    fn more_blocks_than_clues() {
        assert_eq!(lane(&[B], &cells("#x#x....")), vec![B]);
    }

    #[test]
    fn by_totals_and_backups() {
        // Nothing decided, so nothing to go on from the grid. The rows fill 4.
        let states = vec![vec![State::Undecided; 3]; 3];
        let cols = vec![vec![1], vec![B], vec![1]];
        let rows = vec![vec![2], vec![1], vec![1]];
        let repaired = repair(&cols, &rows, &[], &[], &[], &[], &states);
        assert_eq!(repaired.cols, vec![vec![1], vec![2], vec![1]]);
        assert_eq!(repaired.filled, 1);
        assert_eq!(repaired.labels, vec![(true, 1, 0, 2)]);

        // Two left: no help from the totals, so back to the first reading where there was one.
        // That leaves only one, for the totals. (The rows fill 5.)
        let cols = vec![vec![1], vec![B], vec![B]];
        let rows = vec![vec![2], vec![2], vec![1]];
        let backups = vec![None, Some(vec![3]), None];
        let repaired = repair(&cols, &rows, &[], &[], &backups, &[], &states);
        assert_eq!(repaired.cols, vec![vec![1], vec![3], vec![1]]);
        assert_eq!(repaired.filled, 1);
    }
}
