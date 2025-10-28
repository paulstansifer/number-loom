// They're used in tests, but it can't see that.
#![allow(unused_macros, dead_code)]

use std::{fmt::Debug, u32};

use crate::puzzle::{BACKGROUND, Clue, Color, Puzzle};
use anyhow::{Context, bail};
use colored::{ColoredString, Colorize};
use ndarray::{ArrayView1, ArrayViewMut1};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum SolveMode {
    // Listed in order from quickest to most comprehensive:
    Skim,
    Settle,
    Scrub,
}

impl SolveMode {
    pub fn all() -> &'static [SolveMode] {
        &[SolveMode::Skim, SolveMode::Settle, SolveMode::Scrub]
    }

    pub fn name(self) -> &'static str {
        match self {
            SolveMode::Skim => "skim",
            SolveMode::Settle => "settle",
            SolveMode::Scrub => "scrub",
        }
    }

    pub fn colorized_name(self) -> ColoredString {
        match self {
            SolveMode::Skim => self.name().green(),
            SolveMode::Settle => self.name().yellow(),
            SolveMode::Scrub => self.name().red(),
        }
    }

    pub fn ch(self) -> char {
        match self {
            SolveMode::Skim => '-',
            SolveMode::Settle => '.',
            SolveMode::Scrub => '+',
        }
    }

    pub fn prev(self) -> Option<SolveMode> {
        match self {
            SolveMode::Skim => None,
            SolveMode::Settle => Some(SolveMode::Skim),
            SolveMode::Scrub => Some(SolveMode::Settle),
        }
    }

    pub fn next(self) -> Option<SolveMode> {
        match self {
            SolveMode::Skim => Some(SolveMode::Settle),
            SolveMode::Settle => Some(SolveMode::Scrub),
            SolveMode::Scrub => None,
        }
    }

    pub fn first() -> SolveMode {
        SolveMode::Skim
    }

    pub fn last() -> SolveMode {
        SolveMode::Scrub
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ModeMap<T> {
    pub skim: T,
    pub settle: T,
    pub scrub: T,
}

impl<T: Clone> ModeMap<T> {
    pub fn new_uniform(value: T) -> ModeMap<T> {
        ModeMap {
            skim: value.clone(),
            settle: value.clone(),
            scrub: value,
        }
    }
}

impl<T: std::fmt::Display> std::fmt::Display for ModeMap<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for mode in SolveMode::all() {
            // In practice, we know this is a count so (HACK) pluralize:
            write!(f, "{}s: {: >6}", mode.name(), self[*mode])?;
            if *mode != SolveMode::last() {
                write!(f, "  ")?;
            }
        }
        Ok(())
    }
}

impl<T> std::ops::Index<SolveMode> for ModeMap<T> {
    type Output = T;

    fn index(&self, index: SolveMode) -> &Self::Output {
        match index {
            SolveMode::Skim => &self.skim,
            SolveMode::Settle => &self.settle,
            SolveMode::Scrub => &self.scrub,
        }
    }
}

impl<T> std::ops::IndexMut<SolveMode> for ModeMap<T> {
    fn index_mut(&mut self, index: SolveMode) -> &mut Self::Output {
        match index {
            SolveMode::Skim => &mut self.skim,
            SolveMode::Settle => &mut self.settle,
            SolveMode::Scrub => &mut self.scrub,
        }
    }
}

// We might want to switch from `Result<>` to `Option<>`, because currently scrubbing generates and
// discards a lot of error text!

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    possible_color_mask: u32,
}

impl Debug for Cell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_known() {
            write!(f, "[{}]", self.unwrap_color().0)
        } else {
            write!(f, "<{:08b}>", self.possible_color_mask)
        }
    }
}

impl Cell {
    pub fn new(puzzle: &Puzzle<impl Clue>) -> Cell {
        let mut res: u32 = 0;
        for color in puzzle.palette.keys() {
            res |= 1 << color.0
        }
        Cell {
            possible_color_mask: res,
        }
    }

    pub fn raw(&self) -> u32 {
        self.possible_color_mask
    }

    /// Not much practical difference between this and `new`.
    pub fn new_anything() -> Cell {
        Cell {
            possible_color_mask: u32::MAX,
        }
    }

    pub fn from_colors(colors: &[Color]) -> Cell {
        let mut res = Self::new_impossible();
        for c in colors {
            res.actually_could_be(*c);
        }
        res
    }

    pub fn from_color(color: Color) -> Cell {
        Cell {
            possible_color_mask: 1 << color.0,
        }
    }

    pub fn is_known(&self) -> bool {
        self.possible_color_mask.is_power_of_two()
    }

    pub fn is_known_to_be(&self, color: Color) -> bool {
        self.possible_color_mask == 1 << color.0
    }

    pub fn can_be(&self, color: Color) -> bool {
        (self.possible_color_mask & 1 << color.0) != 0
    }

    // TODO: this could be a lot more efficient by using a bitmask as an iterator.
    pub fn can_be_iter(&self) -> impl Iterator<Item = Color> + use<> {
        let mut res = vec![];
        for i in 0..32 {
            if self.possible_color_mask & (1 << i) != 0 {
                res.push(Color(i));
            }
        }
        res.into_iter()
    }

    pub fn known_or(&self) -> Option<Color> {
        if !self.is_known() {
            None
        } else {
            Some(Color(self.possible_color_mask.ilog2() as u8))
        }
    }

    /// Returns whether anything new was discovered (or an error if it's impossible)
    pub fn learn(&mut self, color: Color) -> anyhow::Result<bool> {
        if !self.can_be(color) {
            bail!("learned a contradiction");
        }
        let already_known = self.is_known();
        self.possible_color_mask = 1 << color.0;
        Ok(!already_known)
    }

    pub fn learn_intersect(&mut self, possible: Cell) -> anyhow::Result<bool> {
        if self.possible_color_mask & possible.possible_color_mask == 0 {
            bail!("learned a contradiction");
        }
        let orig_mask = self.possible_color_mask;
        self.possible_color_mask &= possible.possible_color_mask;

        Ok(self.possible_color_mask != orig_mask)
    }

    /// Returns whether anything new was discovered (or an error if it's impossible)
    pub fn learn_that_not(&mut self, color: Color) -> anyhow::Result<bool> {
        if self.is_known_to_be(color) {
            bail!("learned a contradiction");
        }
        let already_known = !self.can_be(color);
        self.possible_color_mask &= !(1 << color.0);
        Ok(!already_known)
    }

    /// Doesn't make sense in the grid, but useful for scrubbing.
    pub fn new_impossible() -> Cell {
        Cell {
            possible_color_mask: 0,
        }
    }

    /// Doesn't make sense in the grid, but useful for scrubbing.
    pub fn actually_could_be(&mut self, color: Color) {
        self.possible_color_mask |= 1 << color.0;
    }

    pub fn contradictory(&self) -> bool {
        self.possible_color_mask == 0
    }

    pub fn unwrap_color(&self) -> Color {
        self.known_or().unwrap()
    }
}

fn bg_squares<C: Clue>(cs: &[C], len: u16) -> u16 {
    let mut remaining = len;
    for c in cs {
        remaining -= c.len() as u16;
    }
    remaining
}

#[derive(Clone)]
pub struct ScrubReport {
    pub affected_cells: Vec<usize>,
}

fn learn_cell(
    color: Color,
    lane: &mut ArrayViewMut1<Cell>,
    idx: usize,
    affected_cells: &mut Vec<usize>,
) -> anyhow::Result<()> {
    if lane[idx].learn(color)? {
        affected_cells.push(idx);
    }
    Ok(())
}

fn learn_cell_intersect(
    possibilities: Cell,
    lane: &mut ArrayViewMut1<Cell>,
    idx: usize,
    affected_cells: &mut Vec<usize>,
) -> anyhow::Result<()> {
    if lane[idx].learn_intersect(possibilities)? {
        affected_cells.push(idx);
    }
    Ok(())
}

fn learn_cell_not(
    color: Color,
    lane: &mut ArrayViewMut1<Cell>,
    idx: usize,
    affected_cells: &mut Vec<usize>,
) -> anyhow::Result<()> {
    if lane[idx].learn_that_not(color)? {
        affected_cells.push(idx);
    }
    Ok(())
}

struct ClueAdjIterator<'a, C: Clue> {
    clues: &'a [C],
    i: usize,
}
impl<'a, C: Clue> ClueAdjIterator<'a, C> {
    fn new(clues: &'a [C]) -> ClueAdjIterator<'a, C> {
        ClueAdjIterator { clues, i: 0 }
    }
}

impl<'a, C: Clue> Iterator for ClueAdjIterator<'a, C> {
    type Item = (bool, &'a C, bool);

    fn next(&mut self) -> Option<Self::Item> {
        if self.i == self.clues.len() {
            return None;
        }
        let res = (
            self.i > 0 && self.clues[self.i - 1].must_be_separated_from(&self.clues[self.i]),
            &self.clues[self.i],
            self.i < self.clues.len() - 1
                && self.clues[self.i].must_be_separated_from(&self.clues[self.i + 1]),
        );
        self.i += 1;
        Some(res)
    }
}

///  For example, (1 2 1) with no other constraints gives
///  .] .  .  .]  .  .]
fn packed_extents<C: Clue + Copy>(
    clues: &[C],
    lane: &ArrayViewMut1<Cell>,
    reversed: bool,
) -> anyhow::Result<Vec<usize>> {
    let mut extents: Vec<usize> = vec![];

    let lane_at = |idx: usize| -> Cell {
        if reversed {
            lane[lane.len() - 1 - idx]
        } else {
            lane[idx]
        }
    };
    let clue_at = |idx: usize| -> &C {
        if reversed {
            &clues[clues.len() - 1 - idx]
        } else {
            &clues[idx]
        }
    };
    let clue_color_at = |clue: &C, idx: usize| -> Color {
        if reversed {
            clue.color_at(clue.len() - 1 - idx)
        } else {
            clue.color_at(idx)
        }
    };

    // -- Pack to the left (we've abstracted over `reversed`) --

    let mut pos = 0_usize;
    let mut last_clue: Option<C> = None;
    for clue_idx in 0..clues.len() {
        let clue = clue_at(clue_idx);
        if let Some(last_clue) = last_clue {
            if !reversed {
                if last_clue.must_be_separated_from(clue) {
                    pos += 1;
                }
            } else {
                if clue.must_be_separated_from(&last_clue) {
                    pos += 1;
                }
            }
        }

        // This could be made a little more efficient with the Boyer-Moore algorithm
        let mut placeable = false;
        while !placeable {
            placeable = true;
            for clue_idx in 0..clue.len() {
                let possible_pos = pos + clue_idx;
                if possible_pos >= lane.len() {
                    anyhow::bail!(
                        "clue {clue:?} at {possible_pos} exceeds lane length {}",
                        lane.len()
                    );
                }
                let cur = lane_at(possible_pos);

                if !cur.can_be(clue_color_at(clue, clue_idx)) {
                    pos += 1;
                    placeable = false;
                    break;
                }
            }
        }
        extents.push(pos + clue.len() - 1);
        pos += clue.len();
        last_clue = Some(*clue);
    }

    // TODO: pull out into a separate function!

    // We might be able to do better; are there any orphaned foreground cells off to the right?
    // (so this `.rev()` has nothing to do with `reversed`!)

    let mut cur_extent_idx = extents.len() - 1;
    let mut i = lane.len() - 1;
    loop {
        if !lane_at(i).can_be(BACKGROUND) {
            // We don't check that the affected clue and the cell have the same color!
            // That's okay for this conservative approximation, but also kinda silly.

            // We ought to reel in clues until we get one of the right color, but that's hard.
            // We're also ignoring the effects of known background squares and gaps between blocks /
            // of the same color. Perhaps some kind of recursion is appropriate here!
            if extents[cur_extent_idx] < i {
                // Pull it in!
                extents[cur_extent_idx] = i;
            }
            // Either way, skip past the rest of the postulated foreground cells
            //  and keep looking.

            // Fencepost farm here!
            // Suppose we pulled a clue with width 3 into position 8:
            //  0  1  2  3  4  5  6  7  8  9
            //                   [      #]
            // 8 - 3 = 5 is the next cell we need to examine. But we'll `-= 1` below, so add 1.
            i = extents[cur_extent_idx] + 1 - clue_at(cur_extent_idx).len();
            if cur_extent_idx == 0 {
                break;
            }
            cur_extent_idx -= 1;
        }
        if i == 0 {
            break;
        }
        i -= 1;
    }

    // -- oh, but fix up the return value --

    if reversed {
        extents.reverse();
        for extent in extents.iter_mut() {
            *extent = lane.len() - *extent - 1;
        }
    }

    Ok(extents)
}

/// Packs all clues to their leftmost and rightmost possible locations. If any squares are
/// guaranteed to be inside a clue, that's useful information!
pub fn skim_line<C: Clue + Copy>(
    clues: &[C],
    lane: &mut ArrayViewMut1<Cell>,
) -> anyhow::Result<ScrubReport> {
    let mut affected = Vec::<usize>::new();
    if clues.is_empty() {
        // Special case, so we can safely take the first and last clue.
        for i in 0..lane.len() {
            learn_cell(BACKGROUND, lane, i, &mut affected).context("Empty clue line")?;
        }
        return Ok(ScrubReport {
            affected_cells: affected,
        });
    }

    // Rule out colors that don't appear at all in this line.
    // Saves some scrubbing!
    let mut possible_colors = Cell::from_color(BACKGROUND);
    for c in clues {
        for i in 0..c.len() {
            possible_colors.actually_could_be(c.color_at(i));
        }
    }
    for i in 0..lane.len() {
        learn_cell_intersect(possible_colors, lane, i, &mut affected)?;
    }

    // Now slam the clues back and forth!
    let left_packed_right_extents = packed_extents(clues, &lane, false)?;
    let right_packed_left_extents = packed_extents(clues, &lane, true)?;

    for ((gap_before, clue, gap_after), (left_extent, right_extent)) in ClueAdjIterator::new(clues)
        .zip(
            right_packed_left_extents
                .iter()
                .zip(left_packed_right_extents.iter()),
        )
    {
        if left_extent > right_extent {
            continue; // No overlap
        }
        if (*right_extent - *left_extent + 1) > clue.len() {
            bail!("clue is insufficiently long");
        }

        let clue_wiggle_room = clue.len() - 1 - (*right_extent - *left_extent);

        for idx in (*left_extent)..=(*right_extent) {
            let mut clue_cell = Cell::new_impossible();
            for wiggle_idx in 0..=clue_wiggle_room {
                clue_cell.actually_could_be(clue.color_at(idx - *left_extent + wiggle_idx));
            }

            learn_cell_intersect(clue_cell, lane, idx, &mut affected).context(format!(
                "overlap: clue {:?} at {}. {:?} -> {:?}",
                clue, idx, lane[idx], clue_cell
            ))?;
        }

        // TODO: this seems to still be necessary, despite the background inference below!
        // Figure out why.
        if (*right_extent as i16 - *left_extent as i16) + 1 == clue.len() as i16 {
            if gap_before {
                learn_cell(BACKGROUND, lane, left_extent - 1, &mut affected)
                    .context(format!("gap before: {:?}", clue))?;
            }
            if gap_after {
                learn_cell(BACKGROUND, lane, right_extent + 1, &mut affected)
                    .context(format!("gap after: {:?}", clue))?;
            }
        }
    }

    // TODO: `packed_extents` should just return both extents of each block.
    let right_packed_right_extents = right_packed_left_extents
        .iter()
        .zip(clues.iter())
        .map(|(extent, clue)| extent + clue.len() - 1);
    let left_packed_left_extents = left_packed_right_extents
        .iter()
        .zip(clues.iter())
        .map(|(extent, clue)| extent + 1 - clue.len());

    // Similarly, are there squares between adjacent blocks that can't be hit (must be background)?
    // I learned you can do this from `pbnsolve`.
    for (right_extent_prev, left_extent) in
        right_packed_right_extents.zip(left_packed_left_extents.skip(1))
    {
        if left_extent == 0 {
            continue;
        }
        for idx in (right_extent_prev + 1)..=(left_extent - 1) {
            learn_cell(BACKGROUND, lane, idx, &mut affected).context(format!(
                "empty between skimmed clues: idx {}, clues: {:?}",
                idx, clues
            ))?;
        }
    }

    let leftmost = left_packed_right_extents[0] as i16 - clues[0].len() as i16;
    let rightmost = right_packed_left_extents.last().unwrap() + clues.last().unwrap().len();

    for i in 0..=leftmost {
        learn_cell(BACKGROUND, lane, i as usize, &mut affected).context(format!("lopen: {}", i))?;
    }
    for i in rightmost..lane.len() {
        learn_cell(BACKGROUND, lane, i, &mut affected).context(format!("ropen: {}", i))?;
    }

    Ok(ScrubReport {
        affected_cells: affected,
    })
}

pub fn settle_line<C: Clue + Copy>(
    clues: &[C],
    lane: &mut ArrayViewMut1<Cell>,
) -> anyhow::Result<ScrubReport> {
    let mut affected = Vec::<usize>::new();
    if clues.is_empty() {
        return Ok(ScrubReport {
            affected_cells: affected,
        });
    }

    let left_packed_right_extents = packed_extents(clues, &lane, false)?;
    let right_packed_left_extents = packed_extents(clues, &lane, true)?;

    for i in 0..clues.len() {
        let clue = &clues[i];
        let right_extent = left_packed_right_extents[i];
        let left_extent = right_packed_left_extents[i];

        if right_extent < left_extent {
            continue; // Impossible placement, skip
        }

        let is_fixed = (right_extent - left_extent + 1) == clue.len();

        if is_fixed {
            // Check before
            if left_extent > 0 {
                if i == 0 || clues[i - 1].must_be_separated_from(clue) {
                    learn_cell(BACKGROUND, lane, left_extent - 1, &mut affected)?;
                }
            }
            // Check after
            if right_extent < lane.len() - 1 {
                if i == clues.len() - 1 || clue.must_be_separated_from(&clues[i + 1]) {
                    learn_cell(BACKGROUND, lane, right_extent + 1, &mut affected)?;
                }
            }
        }
    }

    Ok(ScrubReport {
        affected_cells: affected,
    })
}

pub fn skim_heuristic<C: Clue>(clues: &[C], lane: ArrayView1<Cell>) -> i32 {
    if clues.is_empty() {
        return 1000; // Can solve it right away!
    }
    let mut longest_foregroundable_span = 0;
    let mut cur_foregroundable_span = 0;

    for cell in lane {
        if !cell.is_known_to_be(BACKGROUND) {
            cur_foregroundable_span += 1;
            longest_foregroundable_span =
                std::cmp::max(cur_foregroundable_span, longest_foregroundable_span);
        } else {
            cur_foregroundable_span = 0;
        }
    }

    let total_clue_length = clues.iter().map(|c| c.len() as u16).sum::<u16>();

    let longest_clue = clues.iter().map(|c| c.len() as u16).max().unwrap();

    let edge_bonus = if !lane.first().unwrap().is_known_to_be(BACKGROUND) {
        2
    } else {
        0
    } + if !lane.last().unwrap().is_known_to_be(BACKGROUND) {
        2
    } else {
        0
    };

    (total_clue_length + longest_clue) as i32 - longest_foregroundable_span + edge_bonus
}

pub fn scrub_line<C: Clue + Clone + Copy>(
    cs: &[C],
    lane: &mut ArrayViewMut1<Cell>,
) -> anyhow::Result<ScrubReport> {
    let mut res = ScrubReport {
        affected_cells: vec![],
    };

    for i in 0..lane.len() {
        if lane[i].is_known() {
            continue;
        }

        for color in lane[i].can_be_iter() {
            let mut hypothetical_lane = lane.to_owned();

            hypothetical_lane[i] = Cell::from_color(color);

            match skim_line(cs, &mut hypothetical_lane.view_mut()) {
                Ok(_) => { /* no luck: no contradiction */ }
                Err(err) => {
                    // `color` is impossible here; we've learned something!
                    // Note that this isn't an error!
                    learn_cell_not(color, lane, i, &mut res.affected_cells)
                        .context(format!("scrub contradiction [{}] at {}", err, i))?;
                }
            }
        }
    }

    Ok(res)
}

pub fn scrub_heuristic<C: Clue>(clues: &[C], lane: ArrayView1<Cell>) -> i32 {
    let mut foreground_cells: i32 = 0;
    // If `space_taken == lane.len()`, the line is immediately solvable with no other knowledge.
    let mut space_taken: i32 = 0;
    let mut longest_clue: i32 = 0;
    let mut last_clue: Option<C> = None;
    for c in clues {
        foreground_cells += c.len() as i32;
        space_taken += c.len() as i32;
        if let Some(last_clue) = last_clue {
            if last_clue.must_be_separated_from(c) {
                // We need to leave a space between these clues.
                space_taken += 1;
            }
        }

        longest_clue = std::cmp::max(longest_clue, c.len() as i32);
        last_clue = Some(*c);
    }
    let longest_clue = longest_clue;
    let space_taken = space_taken;

    let known_background_cells = lane
        .into_iter()
        .filter(|cell| cell.is_known_to_be(BACKGROUND))
        .count() as i32;

    let unknown_cells = lane.into_iter().filter(|cell| !cell.is_known()).count() as i32;

    let known_foreground_cells = lane.len() as i32 - unknown_cells - known_background_cells;

    // scrubbing colored squares back and forth is likely to show colored squares if this is high:
    let density = space_taken - known_foreground_cells + longest_clue - clues.len() as i32;

    let mut known_foreground_chunks: i32 = 0;
    let mut in_a_foreground_chunk = false;
    for cell in lane {
        if !cell.can_be(BACKGROUND) {
            if !in_a_foreground_chunk {
                known_foreground_chunks += 1;
            }
            in_a_foreground_chunk = true;
        } else {
            in_a_foreground_chunk = false;
        }
    }

    let unknown_background_cells = (lane.len() as i32 - foreground_cells) - known_background_cells;

    // Matching contiguous foreground cells to clues is likely to show background squares if this
    // is high:
    // > 0 is very good, 0 is still good, -1 is alright, -2 is probably not worth looking at.
    let excess_chunks = if known_foreground_cells > 0 {
        known_foreground_chunks - clues.len() as i32
    } else {
        -2
    };

    density + std::cmp::max(0, unknown_background_cells * (excess_chunks + 2) / 2)
}

pub fn exhaust_line<C: Clue + Clone + Copy>(
    cs: &[C],
    lane: &mut ArrayViewMut1<Cell>,
) -> anyhow::Result<ScrubReport> {
    if cs.is_empty() {
        let mut affected_cells = vec![];

        for i in 0..lane.len() {
            learn_cell(BACKGROUND, lane, i, &mut affected_cells)?
        }

        return Ok(ScrubReport { affected_cells });
    }

    let total_slack = bg_squares(cs, lane.len() as u16) as usize;

    // We want to store all possible locations for all the clues.
    // As an optimization, to keep the table smaller, instead of storing an index into the lane,
    // we store the total gap so far (as if all clues were zero-width). We add `clue_len_so_far`
    // to recover the actual index.

    // The "edge" columns are handled by an "if" rather than being stored in the table
    //        Edge | A | B | C | Edge
    // Gap 0   *   | * | * | - | -
    // Gap 1   -   | * | * | * | -
    // Gap 2   -   | - | * | * | *

    let mut reachable = vec![vec![false; total_slack + 1]; cs.len()];

    // Flood-fill left-reachability:
    let mut clue_len_so_far = 0;
    for clue_idx in 0..cs.len() {
        let clue = &cs[clue_idx];
        // Look at all of the places the previous clue might've been located
        for pfx_gap in 0..=total_slack {
            if clue_idx == 0 {
                if pfx_gap != 0 {
                    continue; // The left edge is always at 0.
                }
            } else if !reachable[clue_idx - 1][pfx_gap] {
                continue; // Previous clue can't be there
            }
            for new_gap in pfx_gap..=total_slack {
                // Try to place the gap before this clue and the clue color itself
                let gap_placeable = (pfx_gap..new_gap)
                    .all(|g_idx| lane[clue_len_so_far + g_idx].can_be(BACKGROUND));
                let color_placeable = (0..clue.len()).all(|clue_cell_idx| {
                    lane[clue_len_so_far + new_gap + clue_cell_idx]
                        .can_be(clue.color_at(clue_cell_idx))
                });
                let consec_placeable = clue_idx == 0
                    || !cs[clue_idx - 1].must_be_separated_from(&cs[clue_idx])
                    || new_gap > pfx_gap;
                if gap_placeable && color_placeable && consec_placeable {
                    reachable[clue_idx][new_gap as usize] = true;
                }
            }
        }
        clue_len_so_far += clue.len();
    }

    let mut superposition = vec![Cell::new_impossible(); lane.len()];

    // Flood-fill right-reachability, intersected with existing reachability:
    // `clue_len_so_far` is correct; now we'll subtract it back to 0.
    for clue_idx in (0..cs.len()).rev() {
        let clue = &cs[clue_idx];

        // Temporary, to be intersected with `reachable`
        let mut both_reachable = vec![false; total_slack + 1];

        let mut max_reachable_suffix = 0;
        for gap_sfx in 0..=total_slack {
            if clue_idx == cs.len() - 1 {
                if gap_sfx != total_slack {
                    continue; // The right edge is always after all the background squares.
                }
            } else if !reachable[clue_idx + 1][gap_sfx] {
                continue; // Clue to the right couldn't be there
            }
            for new_gap in 0..=gap_sfx {
                if !reachable[clue_idx][new_gap] {
                    continue; // Spot not reachable from the LHS, so not worth reaching for.
                }
                // Try to place the clue color and the gap AFTER the clue.
                let clue_placeable = (0..clue.len()).all(|clue_cell_idx| {
                    lane[clue_len_so_far - clue.len() + new_gap + clue_cell_idx]
                        .can_be(clue.color_at(clue_cell_idx))
                });
                let gap_placeable = (new_gap..gap_sfx)
                    .all(|g_idx| lane[clue_len_so_far + g_idx].can_be(BACKGROUND));
                let consec_placeable = clue_idx == cs.len() - 1
                    || !cs[clue_idx].must_be_separated_from(&cs[clue_idx + 1])
                    || new_gap < gap_sfx;

                if gap_placeable && clue_placeable && consec_placeable {
                    both_reachable[new_gap as usize] = true;
                    max_reachable_suffix = gap_sfx;
                }
            }
        }
        for new_gap in 0..=total_slack {
            if both_reachable[new_gap] {
                // Reachable in both directions! Record it:
                for clue_cell_idx in 0..clue.len() {
                    superposition[clue_len_so_far - clue.len() + new_gap + clue_cell_idx]
                        .actually_could_be(clue.color_at(clue_cell_idx));
                }
                for g_idx in new_gap..max_reachable_suffix {
                    superposition[clue_len_so_far + g_idx].actually_could_be(BACKGROUND);
                }
            } else {
                reachable[clue_idx][new_gap] = false;
            }
        }

        clue_len_so_far -= clue.len();
    }

    // We need to handle the first gap, since the RHS-to-LHS pass doesn't look at it.
    for first_gap in (0..=total_slack).rev() {
        if reachable[0][first_gap] {
            for g_idx in 0..first_gap {
                superposition[g_idx].actually_could_be(BACKGROUND);
            }
            break; // Only need to record the longest possible gap
        }
    }

    let mut affected_cells = vec![];

    for i in 0..lane.len() {
        learn_cell_intersect(superposition[i], lane, i, &mut affected_cells)?;
    }

    Ok(ScrubReport { affected_cells })
}

pub fn filter_report_by_color(
    report: &mut ScrubReport,
    orig_lane: &[Cell],
    new_lane: &mut ArrayViewMut1<Cell>,
    color: Color,
) {
    let mut new_affected_cells = vec![];
    for &idx in &report.affected_cells {
        if new_lane[idx].is_known_to_be(color) {
            new_affected_cells.push(idx);
        } else {
            new_lane[idx] = orig_lane[idx];
        }
    }
    report.affected_cells = new_affected_cells;
}

macro_rules! nc {
    ($color:expr, $count:expr) => {
        crate::puzzle::Nono {
            color: $color.unwrap_color(),
            count: $count,
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::puzzle::{Nono, Triano};

    // Uses `Cell` everywhere, even in the clues, for simplicity, even though clues have to be one
    // specific_color
    fn nc(color: Cell, count: u16) -> Nono {
        Nono {
            color: color.unwrap_color(),
            count,
        }
    }

    fn parse_color(c: char) -> Color {
        match c {
            '⬜' => Color(0),
            '⬛' => Color(1),
            '🟥' => Color(2),
            '🟩' => Color(3),
            '🮞' => Color(4),
            '🮟' => Color(5),
            _ => panic!("unknown color: {}", c),
        }
    }

    fn n(spec: &str) -> Vec<Nono> {
        let mut res = vec![];
        for chunk in spec.split_whitespace() {
            let mut chunk_chars = chunk.chars();
            let color = parse_color(chunk_chars.next().unwrap());
            let count = chunk_chars.collect::<String>().parse::<u16>().unwrap();
            res.push(Nono { color, count });
        }
        res
    }

    fn tri(spec: &str) -> Vec<Triano> {
        use crate::puzzle::Triano;

        let mut res = vec![];
        for chunk in spec.split_whitespace() {
            let mut clue = Triano {
                front_cap: None,
                body_color: Color(1),
                body_len: 0,
                back_cap: None,
            };
            if chunk.starts_with('🮞') {
                clue.front_cap = Some(parse_color('🮞'));
            }
            if chunk.ends_with('🮟') {
                clue.back_cap = Some(parse_color('🮟'));
            }
            clue.body_color = parse_color('⬛');
            clue.body_len = chunk
                .trim_start_matches('🮞')
                .trim_end_matches('🮟')
                .parse()
                .unwrap();

            res.push(clue);
        }
        res
    }

    fn l(spec: &str) -> ndarray::Array1<Cell> {
        let mut res = vec![];
        for cell_spec in spec.split_whitespace() {
            if cell_spec == "🔳" {
                let mut bw = Cell::new_impossible();
                bw.actually_could_be(Color(0));
                bw.actually_could_be(Color(1));
                res.push(bw);
                continue;
            }

            let mut cell = Cell::new_impossible();
            for c in cell_spec.chars() {
                cell.actually_could_be(parse_color(c));
            }
            res.push(cell);
        }
        ndarray::arr1(&res)
    }

    fn test_exhaust<C: Clue>(clues: Vec<C>, init: &str) -> ndarray::Array1<Cell> {
        let mut working_line = l(init);
        exhaust_line(
            &clues,
            &mut working_line.rows_mut().into_iter().next().unwrap(),
        )
        .unwrap();
        working_line
    }

    fn test_scrub<C: Clue>(clues: Vec<C>, init: &str) -> ndarray::Array1<Cell> {
        let mut working_line = l(init);
        scrub_line(
            &clues,
            &mut working_line.rows_mut().into_iter().next().unwrap(),
        )
        .unwrap();
        working_line
    }

    fn test_skim<C: Clue>(clues: Vec<C>, init: &str) -> ndarray::Array1<Cell> {
        let mut working_line = l(init);
        skim_line(
            &clues,
            &mut working_line.rows_mut().into_iter().next().unwrap(),
        )
        .unwrap();
        working_line
    }

    fn test_settle<C: Clue>(clues: Vec<C>, init: &str) -> ndarray::Array1<Cell> {
        let mut working_line = l(init);
        settle_line(
            &clues,
            &mut working_line.rows_mut().into_iter().next().unwrap(),
        )
        .unwrap();
        working_line
    }

    #[test]
    fn scrub_test() {
        assert_eq!(test_scrub(n("⬛1"), "🔳 🔳 🔳 🔳"), l("🔳 🔳 🔳 🔳"));

        assert_eq!(test_scrub(n("⬛1"), "⬜ 🔳 🔳 🔳"), l("⬜ 🔳 🔳 🔳"));

        assert_eq!(test_scrub(n("⬛1 ⬛2"), "🔳 🔳 🔳 🔳"), l("⬛ ⬜ ⬛ ⬛"));

        assert_eq!(test_scrub(n("⬛1"), "🔳 🔳 ⬛ 🔳"), l("⬜ ⬜ ⬛ ⬜"));

        assert_eq!(test_scrub(n("⬛3"), "🔳 🔳 🔳 🔳"), l("🔳 ⬛ ⬛ 🔳"));

        assert_eq!(test_scrub(n("⬛3"), "🔳 ⬛ 🔳 🔳 🔳"), l("🔳 ⬛ ⬛ 🔳 ⬜"));

        assert_eq!(
            test_scrub(n("⬛2 ⬛2"), "🔳 🔳 🔳 🔳 🔳"),
            l("⬛ ⬛ ⬜ ⬛ ⬛")
        );

        // Different colors don't need separation, so we don't know as much:
        assert_eq!(
            test_scrub(n("🟥2 ⬛2"), "🟥⬛⬜ 🟥⬛⬜ 🟥⬛⬜ 🟥⬛⬜ 🟥⬛⬜"),
            l("🟥⬜ 🟥 🟥⬛⬜ ⬛ ⬛⬜")
        );
    }

    #[test]
    fn exhaust_test() {
        assert_eq!(test_exhaust(n("⬛1"), "🔳 🔳 🔳 🔳"), l("🔳 🔳 🔳 🔳"));

        assert_eq!(test_exhaust(n("⬛1"), "⬜ 🔳 🔳 🔳"), l("⬜ 🔳 🔳 🔳"));

        assert_eq!(test_exhaust(n("⬛1 ⬛2"), "🔳 🔳 🔳 🔳"), l("⬛ ⬜ ⬛ ⬛"));

        assert_eq!(test_exhaust(n("⬛1"), "🔳 🔳 ⬛ 🔳"), l("⬜ ⬜ ⬛ ⬜"));

        assert_eq!(test_exhaust(n("⬛3"), "🔳 🔳 🔳 🔳"), l("🔳 ⬛ ⬛ 🔳"));

        assert_eq!(
            test_exhaust(n("⬛3"), "🔳 ⬛ 🔳 🔳 🔳"),
            l("🔳 ⬛ ⬛ 🔳 ⬜")
        );

        assert_eq!(
            test_exhaust(n("⬛2 ⬛2"), "🔳 🔳 🔳 🔳 🔳"),
            l("⬛ ⬛ ⬜ ⬛ ⬛")
        );

        assert_eq!(
            test_exhaust(n("⬛2 ⬛2"), "🔳 🔳 🔳 🔳 🔳 🔳"),
            l("🔳 ⬛ 🔳 🔳 ⬛ 🔳")
        );

        // Different colors don't need separation, so we don't know as much:
        assert_eq!(
            test_exhaust(n("🟥2 ⬛2"), "🟥⬛⬜ 🟥⬛⬜ 🟥⬛⬜ 🟥⬛⬜ 🟥⬛⬜"),
            l("🟥⬜ 🟥 🟥⬛⬜ ⬛ ⬛⬜")
        );
    }

    #[test]
    fn skim_test() {
        assert_eq!(test_skim(n("⬛1"), "🔳 🔳 🔳 🔳"), l("🔳 🔳 🔳 🔳"));

        assert_eq!(test_skim(n("⬛1"), "⬜ 🔳 🔳 🔳"), l("⬜ 🔳 🔳 🔳"));

        assert_eq!(test_skim(n("⬛3"), "🔳 🔳 🔳 🔳"), l("🔳 ⬛ ⬛ 🔳"));

        assert_eq!(test_skim(n("⬛2 ⬛1"), "🔳 🔳 🔳 🔳"), l("⬛ ⬛ ⬜ ⬛"));

        assert_eq!(test_skim(n("⬛1 ⬛2"), "🔳 🔳 🔳 🔳"), l("⬛ ⬜ ⬛ ⬛"));

        assert_eq!(
            test_skim(n("⬛2"), "🔳 🔳 🔳 🔳 🔳 ⬛ ⬛ 🔳"),
            l("⬜ ⬜ ⬜ ⬜ ⬜ ⬛ ⬛ ⬜")
        );

        assert_eq!(test_skim(n("⬛1"), "🔳 🔳 ⬛ 🔳"), l("⬜ ⬜ ⬛ ⬜"));

        assert_eq!(test_skim(n("⬛3"), "🔳 ⬛ 🔳 🔳 🔳"), l("🔳 ⬛ ⬛ 🔳 ⬜"));

        assert_eq!(
            test_skim(n("⬛2 ⬛2"), "🔳 🔳 🔳 🔳 🔳"),
            l("⬛ ⬛ ⬜ ⬛ ⬛")
        );

        // Different colors don't need separation, so we don't know as much:
        assert_eq!(
            test_skim(n("🟥2 ⬛2"), "🟥⬛⬜ 🟥⬛⬜ 🟥⬛⬜ 🟥⬛⬜ 🟥⬛⬜"),
            l("🟥⬛⬜ 🟥 🟥⬛⬜ ⬛ 🟥⬛⬜")
        );

        // Test with longer clues
        assert_eq!(
            test_skim(n("⬛7"), "🔳 🔳 🔳 🔳 🔳 🔳 🔳 🔳 🔳 🔳"),
            l("🔳 🔳 🔳 ⬛ ⬛ ⬛ ⬛ 🔳 🔳 🔳")
        );

        // Test with more clues per line
        assert_eq!(
            test_skim(n("⬛1 ⬛1 ⬛1 ⬛1"), "🔳 🔳 🔳 🔳 🔳 🔳 🔳"),
            l("⬛ ⬜ ⬛ ⬜ ⬛ ⬜ ⬛")
        );

        assert_eq!(
            test_skim(n("⬛6"), "⬛ ⬛ 🔳 🔳 ⬛ ⬛"),
            l("⬛ ⬛ ⬛ ⬛ ⬛ ⬛")
        );
    }

    #[test]
    fn skim_tri_test() {
        // Perhaps skimming should figure out things based on the known ends of clues?
        assert_eq!(
            test_skim(tri("🮞1"), "🮞⬛🮟⬜ 🮞⬛🮟⬜ 🮞⬛🮟⬜ 🮞⬛🮟⬜"),
            l("🮞⬛⬜ 🮞⬛⬜ 🮞⬛⬜ 🮞⬛⬜")
        );

        assert_eq!(
            test_skim(tri("🮞2"), "🮞⬛🮟⬜ 🮞⬛🮟⬜ 🮞⬛🮟⬜ 🮞⬛🮟⬜"),
            l("🮞⬛⬜ 🮞⬛ ⬛ 🮞⬛⬜")
        );
    }

    #[test]
    fn settle_test() {
        assert_eq!(
            test_settle(n("⬛1 ⬛1"), "⬛ 🔳 🔳 🔳 ⬛"),
            l("⬛ ⬜ 🔳 ⬜ ⬛")
        );
    }

    macro_rules! heur {
    ([$($color:expr, $count:expr);*] $($state:expr),*) => {
        {
            let initial = ndarray::arr1(&[ $($state),* ]);
            scrub_heuristic(
                &vec![ $( crate::puzzle::Nono { color: $color.unwrap_color(), count: $count} ),* ],
                initial.rows().into_iter().next().unwrap())
        }
    };
}

    // TODO: actually test the Triano case!

    #[test]
    fn heuristic_examples() {
        let x = Cell::new_anything();
        let w = Cell::from_color(Color(0));
        let b = Cell::from_color(Color(1));

        assert_eq!(heur!([b, 1]  x, x, x, x), 1);
        assert_eq!(heur!([b, 1]  w, x, x, x), 1);
        assert_eq!(heur!([b, 2]  w, w, x, x), 3);
        assert_eq!(heur!([b, 1; b, 2]  x, x, x, x), 4);
        assert_eq!(heur!([b, 1]  x, x, b, x), 3);
        assert_eq!(heur!([b, 3]  x, x, x, x), 5);
        assert_eq!(heur!([b, 3]  x, b, x, x, x), 6);

        assert_eq!(
            heur!([b, 10]  x, x, x, x, x, x, x, x, x, x, x, x, x, x, x),
            19
        );
        assert_eq!(
            heur!([b, 3]  x, x, x, x, x, x, x, x, x, x, x, x, x, x, x),
            5
        );
        assert_eq!(
            heur!([b, 3]  x, x, x, x, b, x, x, x, x, x, x, x, x, x, x),
            16
        );
    }

    #[test]
    fn filter_report() {
        let mut rep = ScrubReport {
            affected_cells: vec![0, 2, 4],
        };
        let orig = l("🟥⬛⬜ 🟥⬛⬜ 🟥⬛⬜ 🟥⬛⬜ 🟥⬛⬜ 🟥 ⬛ ⬜");
        let mut solved = l("🟥 🟥⬛⬜ ⬛⬜ 🟥⬛⬜ ⬜ 🟥 ⬛ ⬜");
        filter_report_by_color(
            &mut rep,
            &orig.iter().cloned().collect::<Vec<_>>(),
            &mut solved.view_mut(),
            BACKGROUND,
        );

        assert_eq!(rep.affected_cells, vec![4]);
        assert_eq!(solved, l("🟥⬛⬜ 🟥⬛⬜ 🟥⬛⬜ 🟥⬛⬜ ⬜ 🟥 ⬛ ⬜"));
    }
}
