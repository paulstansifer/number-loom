// They're used in tests, but it can't see that.
#![allow(unused_macros, dead_code)]

use std::{fmt::Debug, u32};

use crate::puzzle::{BACKGROUND, Clue, Color};
use anyhow::{Context, bail};
use colored::{ColoredString, Colorize};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum SolveMode {
    // Listed in order from quickest to most comprehensive:
    Skim,
    Scrub,
}

impl SolveMode {
    pub fn all() -> &'static [SolveMode] {
        &[SolveMode::Skim, SolveMode::Scrub]
    }

    pub fn name(self) -> &'static str {
        match self {
            SolveMode::Skim => "skim",
            SolveMode::Scrub => "scrub",
        }
    }

    pub fn colorized_name(self) -> ColoredString {
        match self {
            SolveMode::Skim => self.name().green(),
            SolveMode::Scrub => self.name().red(),
        }
    }

    pub fn ch(self) -> char {
        match self {
            SolveMode::Skim => '-',
            SolveMode::Scrub => '+',
        }
    }

    pub fn prev(self) -> Option<SolveMode> {
        match self {
            SolveMode::Skim => None,
            SolveMode::Scrub => Some(SolveMode::Skim),
        }
    }

    pub fn next(self) -> Option<SolveMode> {
        match self {
            SolveMode::Skim => Some(SolveMode::Scrub),
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
    pub scrub: T,
}

impl<T: Clone> ModeMap<T> {
    pub fn new_uniform(value: T) -> ModeMap<T> {
        ModeMap {
            skim: value.clone(),
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
            SolveMode::Scrub => &self.scrub,
        }
    }
}

impl<T> std::ops::IndexMut<SolveMode> for ModeMap<T> {
    fn index_mut(&mut self, index: SolveMode) -> &mut Self::Output {
        match index {
            SolveMode::Skim => &mut self.skim,
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
    pub fn new(palette: &crate::puzzle::Palette) -> Cell {
        let mut res: u32 = 0;
        for color in palette.keys() {
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

/// How many of a lane's squares the clues leave over for background, or `None` if the clues are
/// too long to fit in it at all. Callers must handle the `None`: an over-long clue list is an
/// ordinary contradiction (it is what a bad guess looks like), not a caller bug.
fn bg_squares<C: Clue>(cs: &[C], len: u16) -> Option<u16> {
    let mut remaining = len;
    for c in cs {
        remaining = remaining.checked_sub(c.len() as u16)?;
    }
    Some(remaining)
}

#[derive(Clone)]
pub struct ScrubReport {
    pub affected_cells: Vec<usize>,
}

fn learn_cell(
    color: Color,
    lane: &mut [Cell],
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
    lane: &mut [Cell],
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
    lane: &mut [Cell],
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
    lane: &[Cell],
    reversed: bool,
) -> anyhow::Result<Vec<usize>> {
    if clues.is_empty() {
        return Ok(vec![]);
    }

    let mut extents: Vec<usize> = Vec::with_capacity(clues.len());

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
pub fn skim_line<C: Clue + Copy>(clues: &[C], lane: &mut [Cell]) -> anyhow::Result<ScrubReport> {
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
    // Optimization: check whether `learn_cell_intersect` would do anything in the first place.
    let mut lane_can_be = 0_u32;
    let mut any_impossible = false;
    for cell in lane.iter() {
        lane_can_be |= cell.raw();
        any_impossible |= cell.raw() & possible_colors.raw() == 0;
    }
    // Only allow the colors that are possible for the clue.
    if any_impossible || lane_can_be & !possible_colors.raw() != 0 {
        for i in 0..lane.len() {
            learn_cell_intersect(possible_colors, lane, i, &mut affected)?;
        }
    }

    // Now slam the clues back and forth!
    let left_packed_right_extents = packed_extents(clues, lane, false)?;
    let right_packed_left_extents = packed_extents(clues, lane, true)?;

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

            learn_cell_intersect(clue_cell, lane, idx, &mut affected).with_context(|| {
                format!(
                    "overlap: clue {:?} at {}. {:?} -> {:?}",
                    clue, idx, lane[idx], clue_cell
                )
            })?;
        }

        // TODO: this seems to still be necessary, despite the background inference below!
        // Figure out why.
        if (*right_extent as i16 - *left_extent as i16) + 1 == clue.len() as i16 {
            if gap_before {
                learn_cell(BACKGROUND, lane, left_extent - 1, &mut affected)
                    .with_context(|| format!("gap before: {:?}", clue))?;
            }
            if gap_after {
                learn_cell(BACKGROUND, lane, right_extent + 1, &mut affected)
                    .with_context(|| format!("gap after: {:?}", clue))?;
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
            learn_cell(BACKGROUND, lane, idx, &mut affected).with_context(|| {
                format!(
                    "empty between skimmed clues: idx {}, clues: {:?}",
                    idx, clues
                )
            })?;
        }
    }

    let leftmost = left_packed_right_extents[0] as i16 - clues[0].len() as i16;
    let rightmost = right_packed_left_extents.last().unwrap() + clues.last().unwrap().len();

    for i in 0..=leftmost {
        learn_cell(BACKGROUND, lane, i as usize, &mut affected)
            .with_context(|| format!("lopen: {}", i))?;
    }
    for i in rightmost..lane.len() {
        learn_cell(BACKGROUND, lane, i, &mut affected).with_context(|| format!("ropen: {}", i))?;
    }

    Ok(ScrubReport {
        affected_cells: affected,
    })
}

pub fn settle_line<C: Clue + Copy>(clues: &[C], lane: &mut [Cell]) -> anyhow::Result<ScrubReport> {
    let mut affected = Vec::<usize>::new();

    let left_packed_right_extents = packed_extents(clues, lane, false)?;
    let right_packed_left_extents = packed_extents(clues, lane, true)?;

    let mut prev_known_end = Some(0); // Left edge is known!
    for i in 0..clues.len() {
        let clue = &clues[i];
        let right_extent = left_packed_right_extents[i];
        let left_extent = right_packed_left_extents[i];

        let is_known = (right_extent + 1) == clue.len() + left_extent
            && (left_extent..=right_extent).all(|j| lane[j].is_known());

        if is_known {
            // Separator background before
            if left_extent > 0 {
                if i > 0 && clues[i - 1].must_be_separated_from(clue) {
                    learn_cell(BACKGROUND, lane, left_extent - 1, &mut affected)?;
                }
            }
            // Separator background after
            if right_extent < lane.len() - 1 {
                if i < clues.len() - 1 && clue.must_be_separated_from(&clues[i + 1]) {
                    learn_cell(BACKGROUND, lane, right_extent + 1, &mut affected)?;
                }
            }

            if let Some(prev_end) = prev_known_end {
                for i in prev_end..left_extent {
                    learn_cell(BACKGROUND, lane, i, &mut affected)?;
                }
            }
        }

        if is_known {
            prev_known_end = Some(right_extent + 1);
        } else {
            prev_known_end = None;
        }
    }

    // Right edge is known too:
    if let Some(prev_end) = prev_known_end {
        for i in prev_end..lane.len() {
            learn_cell(BACKGROUND, lane, i, &mut affected)?;
        }
    }

    Ok(ScrubReport {
        affected_cells: affected,
    })
}

pub fn skim_heuristic<C: Clue>(clues: &[C], lane: &[Cell]) -> i32 {
    score_lane(&ClueSummary::new(clues), lane).skim
}

// This is the old "scrub"; we don't use it anymore
pub fn scrub_line<C: Clue + Clone + Copy>(
    cs: &[C],
    lane: &mut [Cell],
) -> anyhow::Result<ScrubReport> {
    let mut res = ScrubReport {
        affected_cells: vec![],
    };

    for i in 0..lane.len() {
        if lane[i].is_known() {
            continue;
        }

        for color in lane[i].can_be_iter() {
            let mut hypothetical_lane = lane.to_vec();

            hypothetical_lane[i] = Cell::from_color(color);

            match skim_line(cs, &mut hypothetical_lane) {
                Ok(_) => { /* no luck: no contradiction */ }
                Err(err) => {
                    // `color` is impossible here; we've learned something!
                    // Note that this isn't an error!
                    learn_cell_not(color, lane, i, &mut res.affected_cells)
                        .with_context(|| format!("scrub contradiction [{}] at {}", err, i))?;
                }
            }
        }
    }

    Ok(res)
}

pub fn scrub_heuristic<C: Clue>(clues: &[C], lane: &[Cell]) -> i32 {
    score_lane(&ClueSummary::new(clues), lane).scrub
}

/// Everything the two heuristics need from a lane's *clues*, saved for performance reasons.
#[derive(Clone, Copy, Debug)]
pub struct ClueSummary {
    /// Total foreground cells the clues account for.
    foreground_cells: i32,
    /// As `foreground_cells`, plus the separators that adjacent same-color clues force. If this
    /// equals the lane length, the line is immediately solvable with no other knowledge.
    space_taken: i32,
    longest_clue: i32,
    count: i32,
}

impl ClueSummary {
    pub fn new<C: Clue>(clues: &[C]) -> ClueSummary {
        let mut foreground_cells: i32 = 0;
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

        ClueSummary {
            foreground_cells,
            space_taken,
            longest_clue,
            count: clues.len() as i32,
        }
    }
}

/// What scoring a lane produces. Both heuristics and the "is this line finished?" test want
/// different tallies of the same cells, so one walk answers all three.
#[derive(Clone, Copy, Debug)]
pub struct LaneScores {
    pub skim: i32,
    pub scrub: i32,
    pub all_known: bool,
}

/// Score a lane for both modes at once. Every caller wants both numbers, and walking the cells
/// is what the scoring costs, so the walk happens once.
pub fn score_lane(summary: &ClueSummary, lane: &[Cell]) -> LaneScores {
    let mut longest_foregroundable_span: i32 = 0;
    let mut cur_foregroundable_span: i32 = 0;
    let mut known_background_cells: i32 = 0;
    let mut unknown_cells: i32 = 0;
    let mut known_foreground_chunks: i32 = 0;
    let mut in_a_foreground_chunk = false;

    for cell in lane {
        if !cell.is_known_to_be(BACKGROUND) {
            cur_foregroundable_span += 1;
            longest_foregroundable_span =
                std::cmp::max(cur_foregroundable_span, longest_foregroundable_span);
        } else {
            cur_foregroundable_span = 0;
            known_background_cells += 1;
        }

        if !cell.is_known() {
            unknown_cells += 1;
        }

        if !cell.can_be(BACKGROUND) {
            if !in_a_foreground_chunk {
                known_foreground_chunks += 1;
            }
            in_a_foreground_chunk = true;
        } else {
            in_a_foreground_chunk = false;
        }
    }

    let skim = if summary.count == 0 {
        1000 // Can solve it right away!
    } else {
        let edge_bonus = if !lane.first().unwrap().is_known_to_be(BACKGROUND) {
            2
        } else {
            0
        } + if !lane.last().unwrap().is_known_to_be(BACKGROUND) {
            2
        } else {
            0
        };

        (summary.foreground_cells + summary.longest_clue) - longest_foregroundable_span + edge_bonus
    };

    let known_foreground_cells = lane.len() as i32 - unknown_cells - known_background_cells;

    // scrubbing colored squares back and forth is likely to show colored squares if this is high:
    let density =
        summary.space_taken - known_foreground_cells + summary.longest_clue - summary.count;

    let unknown_background_cells =
        (lane.len() as i32 - summary.foreground_cells) - known_background_cells;

    // Matching contiguous foreground cells to clues is likely to show background squares if this
    // is high:
    // > 0 is very good, 0 is still good, -1 is alright, -2 is probably not worth looking at.
    let excess_chunks = if known_foreground_cells > 0 {
        known_foreground_chunks - summary.count
    } else {
        -2
    };

    let scrub = density + std::cmp::max(0, unknown_background_cells * (excess_chunks + 2) / 2);

    LaneScores {
        skim,
        scrub,
        all_known: unknown_cells == 0,
    }
}

// This is the new thing we call "scrub" (TODO: make names consistent!)
pub fn exhaust_line<C: Clue + Clone + Copy>(
    cs: &[C],
    lane: &mut [Cell],
) -> anyhow::Result<ScrubReport> {
    if cs.is_empty() {
        let mut affected_cells = vec![];

        for i in 0..lane.len() {
            learn_cell(BACKGROUND, lane, i, &mut affected_cells)?
        }

        return Ok(ScrubReport { affected_cells });
    }

    let Some(total_slack) = bg_squares(cs, lane.len() as u16) else {
        bail!("clues are longer than the lane");
    };
    let total_slack = total_slack as usize;

    // We want to store all possible locations for all the clues.
    // As an optimization, to keep the table smaller, instead of storing an index into the lane,
    // we store the total gap so far (as if all clues were zero-width). We add `clue_len_so_far`
    // to recover the actual index.

    // The "edge" columns are handled by an "if" rather than being stored in the table
    //        Edge | A | B | C | Edge
    // Gap 0   *   | * | * | - | -
    // Gap 1   -   | * | * | * | -
    // Gap 2   -   | - | * | * | *

    // One flat table rather than a `Vec` per clue: `reachable[clue_idx * gap_stride + gap]`.
    // This is on the hot path, and a nested `Vec` costs an allocation per clue every call.
    let gap_stride = total_slack + 1;
    let mut reachable = vec![false; gap_stride * cs.len()];

    // Both flood fills keep asking "does this clue fit here?" repeately, so figure that out for
    // all positions here.
    // `clue_fits[clue_idx * gap_stride + gap]` is whether clue `clue_idx` can occupy the cells
    // that `gap` places it on.
    let mut clue_fits = vec![false; gap_stride * cs.len()];
    let mut clue_len_so_far = 0;
    for (clue_idx, clue) in cs.iter().enumerate() {
        for gap in 0..=total_slack {
            clue_fits[clue_idx * gap_stride + gap] = (0..clue.len()).all(|clue_cell_idx| {
                lane[clue_len_so_far + gap + clue_cell_idx].can_be(clue.color_at(clue_cell_idx))
            });
        }
        clue_len_so_far += clue.len();
    }

    // Flood-fill left-reachability.
    //
    // Rather than testing every (previous gap, new gap) pair, note that the previous gaps that
    // can feed a given `new_gap` are exactly those with no un-backgroundable cell in between —
    // a contiguous range whose ends both only move right as `new_gap` grows. So sweep `new_gap`
    // upwards and keep a running count of how many gaps in that window the previous clue can
    // actually reach; the window only ever needs one visit per gap, not one per pair.
    let mut clue_len_so_far = 0;
    for clue_idx in 0..cs.len() {
        let needs_gap = clue_idx > 0 && cs[clue_idx - 1].must_be_separated_from(&cs[clue_idx]);

        // HACK: get disjoint borrows for `this_row` (mutably) and `prev_row` -> `prev_reachable`
        let (earlier_rows, rest) = reachable.split_at_mut(clue_idx * gap_stride);
        let this_row = &mut rest[..gap_stride];
        let prev_row = clue_idx
            .checked_sub(1)  // First clue has not predecessor...
            .map(|prev| &earlier_rows[prev * gap_stride..][..gap_stride]);
        let prev_reachable /* Fn(usize) -> usize */ = |gap: usize| match prev_row {
            Some(row) => row[gap],
            None => gap == 0, // ...the left edge stands in for it
        };

        // `reachable_in_window` counts the reachable gaps in `lo..added`, and `added` never runs
        // past the largest previous gap that the current `new_gap` would accept.
        let mut lo: usize = 0;
        let mut added: usize = 0;
        let mut reachable_in_window: usize = 0;

        for new_gap in 0..=total_slack {
            // The cell just short of `new_gap` has become part of the gap...
            if new_gap > 0 && !lane[clue_len_so_far + new_gap - 1].can_be(BACKGROUND) {
                // ...and it can't be background, so
                while lo < new_gap {
                    // every previous gap at or below it is ruled out from here on
                    if lo < added && prev_reachable(lo) {
                        reachable_in_window -= 1;
                    }
                    lo += 1;
                }
                added = added.max(lo);
            }

            // A clue that must be separated from its predecessor can't start where the
            // predecessor's gap left off, so it gives up one gap of reach.
            let highest_pfx_gap : Option<usize> = if needs_gap {
                new_gap.checked_sub(1)
            } else {
                Some(new_gap)
            };
            if let Some(highest_pfx_gap) = highest_pfx_gap {
                while added <= highest_pfx_gap {
                    if prev_reachable(added) {
                        reachable_in_window += 1;
                    }
                    added += 1;
                }
            }

            if reachable_in_window > 0 && clue_fits[clue_idx * gap_stride + new_gap] {
                this_row[new_gap] = true;
            }
        }
        clue_len_so_far += cs[clue_idx].len();
    }

    let mut superposition = vec![Cell::new_impossible(); lane.len()];

    // Temporary, to be intersected with `reachable`. Allocated once and cleared per clue.
    let mut both_reachable = vec![false; gap_stride];

    // Flood-fill right-reachability, intersected with existing reachability:
    // `clue_len_so_far` made it to the high-water-mark; now we'll subtract it back to 0.
    for clue_idx in (0..cs.len()).rev() {
        let clue = &cs[clue_idx];
        let needs_gap =
            clue_idx + 1 < cs.len() && cs[clue_idx].must_be_separated_from(&cs[clue_idx + 1]);

        both_reachable.fill(false);

        // The mirror of the left pass's sweep, plus the background cells to record. `lo` is the
        // leftmost gap this clue could sit at while still leaving every cell between it and
        // `gap_sfx` background, and it only moves right as `gap_sfx` grows — as do `first_ok`
        // and `gap_sfx` itself. Because all three ends only move right, both of the ranges
        // written below can pick up where the last one stopped, which is what keeps a clue's
        // whole sweep linear in the slack instead of quadratic.
        let mut lo: usize = 0;
        // The leftmost gap at or after `lo` where this clue both fits and is reachable from the
        // left; the leftmost start any surviving arrangement can have, in other words.
        let mut first_ok: usize = 0;
        let mut marked_up_to: usize = 0;
        let mut painted_up_to: usize = 0;

        for gap_sfx in 0..=total_slack {
            if gap_sfx > 0 && !lane[clue_len_so_far + gap_sfx - 1].can_be(BACKGROUND) {
                lo = gap_sfx;
            }

            if clue_idx == cs.len() - 1 {
                if gap_sfx != total_slack {
                    continue; // The right edge is always after all the background squares.
                }
            } else if !reachable[(clue_idx + 1) * gap_stride + gap_sfx] {
                continue; // Clue to the right couldn't be there
            }

            // As on the left, a clue that must be separated from its neighbour gives up a gap.
            let Some(highest_gap) = (if needs_gap {
                gap_sfx.checked_sub(1)
            } else {
                Some(gap_sfx)
            }) else {
                continue;
            };
            if highest_gap < lo {
                continue;
            }

            // Every gap in `lo..=highest_gap` that the left pass also reached is actually reachable!
            for gap in marked_up_to.max(lo)..=highest_gap {
                if reachable[clue_idx * gap_stride + gap] && clue_fits[clue_idx * gap_stride + gap]
                {
                    both_reachable[gap] = true;
                }
            }
            marked_up_to = marked_up_to.max(highest_gap + 1);

            // Advancing over gaps this clue can't use is always safe: they can't come back.
            first_ok = first_ok.max(lo);
            while first_ok <= highest_gap
                && !(reachable[clue_idx * gap_stride + first_ok]
                    && clue_fits[clue_idx * gap_stride + first_ok])
            {
                first_ok += 1;
            }

            // Cells between the clue's leftmost real spot and `gap_sfx` are ones some
            // arrangement leaves as background.
            if first_ok <= highest_gap {
                for gap in painted_up_to.max(first_ok)..gap_sfx {
                    superposition[clue_len_so_far + gap].actually_could_be(BACKGROUND);
                }
                painted_up_to = painted_up_to.max(gap_sfx);
            }
        }
        for new_gap in 0..=total_slack {
            if both_reachable[new_gap] {
                // TODO: why not do this in the previous loop?
                // Reachable in both directions! Record it:
                for clue_cell_idx in 0..clue.len() {
                    superposition[clue_len_so_far - clue.len() + new_gap + clue_cell_idx]
                        .actually_could_be(clue.color_at(clue_cell_idx));
                }
            } else {
                reachable[clue_idx * gap_stride + new_gap] = false;
            }
        }

        clue_len_so_far -= clue.len();
    }

    // We need to handle the first gap, since the RHS-to-LHS pass doesn't look at it.
    for first_gap in (0..=total_slack).rev() {
        if reachable[first_gap] {
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
    new_lane: &mut [Cell],
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

    fn l(spec: &str) -> Vec<Cell> {
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
        res
    }

    fn test_exhaust<C: Clue>(clues: Vec<C>, init: &str) -> Vec<Cell> {
        let mut working_line = l(init);
        exhaust_line(&clues, &mut working_line).unwrap();
        working_line
    }

    fn test_scrub<C: Clue>(clues: Vec<C>, init: &str) -> Vec<Cell> {
        let mut working_line = l(init);
        scrub_line(&clues, &mut working_line).unwrap();
        working_line
    }

    fn test_skim<C: Clue>(clues: Vec<C>, init: &str) -> Vec<Cell> {
        let mut working_line = l(init);
        skim_line(&clues, &mut working_line).unwrap();
        working_line
    }

    fn test_settle<C: Clue>(clues: Vec<C>, init: &str) -> Vec<Cell> {
        let mut working_line = l(init);
        settle_line(&clues, &mut working_line).unwrap();
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

    /// Clues longer than the lane are a contradiction, not a caller bug: `bg_squares` used to
    /// underflow here, panicking in debug and wrapping to a colossal slack in release.
    #[test]
    fn clues_too_long_for_the_lane_are_an_error() {
        for init in ["🔳 🔳 🔳", "⬜ ⬛ 🔳", "⬛ ⬛ ⬛"] {
            let mut lane = l(init);
            assert!(
                exhaust_line(&n("⬛4"), &mut lane).is_err(),
                "exhaust_line accepted an over-long clue on {init}"
            );

            let mut lane = l(init);
            assert!(
                exhaust_line(&n("⬛2 ⬛3"), &mut lane).is_err(),
                "exhaust_line accepted over-long clues on {init}"
            );

            // `skim_line` already rejected these; make sure it still does.
            let mut lane = l(init);
            assert!(skim_line(&n("⬛4"), &mut lane).is_err());
        }

        // Exactly filling the lane is fine, and leaves no slack at all.
        assert_eq!(test_exhaust(n("⬛3"), "🔳 🔳 🔳"), l("⬛ ⬛ ⬛"));
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
        // TODO: I feel like it shouldn't need the separators around the final clue to get this.
        // Maybe `packed_extents` should be improved in some way?
        assert_eq!(
            test_settle(
                n("⬛1 ⬛3 ⬛2"),
                "🔳 🔳 ⬜ ⬛ ⬛ ⬛ ⬜ 🔳 🔳 ⬜ ⬛ ⬛ ⬜ 🔳"
            ),
            l("🔳 🔳 ⬜ ⬛ ⬛ ⬛ ⬜ ⬜ ⬜ ⬜ ⬛ ⬛ ⬜ ⬜")
        );

        assert_eq!(
            test_settle(n("⬛1 ⬛1"), "⬛ 🔳 🔳 🔳 ⬛"),
            l("⬛ ⬜ ⬜ ⬜ ⬛")
        );

        // Without filled cells, we can't do anything:
        assert_eq!(
            test_settle(n("⬛1 ⬛1 ⬛1"), "🔳 🔳 🔳 🔳 🔳"),
            l("🔳 🔳 🔳 🔳 🔳")
        );

        assert_eq!(test_settle(n(""), "🔳 🔳 🔳 🔳 🔳"), l("⬜ ⬜ ⬜ ⬜ ⬜"));
    }

    macro_rules! heur {
    ([$($color:expr, $count:expr);*] $($state:expr),*) => {
        scrub_heuristic(
            &vec![ $( crate::puzzle::Nono { color: $color.unwrap_color(), count: $count} ),* ],
            &[ $($state),* ])
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
        filter_report_by_color(&mut rep, &orig, &mut solved, BACKGROUND);

        assert_eq!(rep.affected_cells, vec![4]);
        assert_eq!(solved, l("🟥⬛⬜ 🟥⬛⬜ 🟥⬛⬜ 🟥⬛⬜ ⬜ 🟥 ⬛ ⬜"));
    }

    #[test]
    fn observed_error() {
        let clues = n("⬛4 ⬛4");
        let init_str = "🔳 🔳 🔳 🔳 ⬜ ⬛ 🔳 🔳 🔳 🔳 🔳 🔳 🔳 🔳 ";

        let result = test_exhaust(clues, init_str);

        assert!(
            result[6].is_known_to_be(Color(1)),
            "should be black, got {:?}",
            result[6]
        );

        let clues = n("⬛2 ⬛2 ⬛4 ⬛4 ⬛1");
        let init_str = "🔳 🔳 🔳 🔳 🔳 🔳 🔳 🔳 🔳 ⬜ ⬛ 🔳 🔳 🔳 🔳 🔳 🔳 🔳 🔳 🔳 🔳 🔳 🔳 🔳 🔳";

        let result = test_exhaust(clues, init_str);

        assert!(
            result[11].is_known_to_be(Color(1)),
            "should be black, got {:?}",
            result[11]
        );

        let clues = n("⬛1 ⬛4 ⬛4 ⬛2 ⬛2");
        let init_str = "🔳 🔳 🔳 🔳 🔳 🔳 🔳 🔳 🔳 🔳 🔳 🔳 🔳 🔳 ⬛ ⬜ 🔳 🔳 🔳 🔳 🔳 🔳 🔳 🔳 🔳";

        let result = test_exhaust(clues, init_str);

        assert!(
            result[13].is_known_to_be(Color(1)),
            "should be black, got {:?}",
            result[13]
        );
    }
}
