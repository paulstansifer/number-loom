//! Line-solving techniques that are more like what people do.
//!
//! Ironically, unlike everything else in solve/, this one is mostly LLM-written.
//!
//! `src/bin/eval-techniques.rs` compares them against each other.

use crate::puzzle::{BACKGROUND, Clue, Color};
use crate::solve::line_solve::{
    Cell, ScrubReport, learn_cell, learn_cell_intersect, learn_cell_not, skim_line,
    skim_to_place_fixed_clues,
};
use anyhow::bail;
use std::ops::Range;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Technique {
    BackgroundSkim,
    ForegroundSkim,
    Extend,
    ContigSkim,
    DoneBySize,
    FindAHome,
    CannotMerge,
    LongGetLonger,
}

impl Technique {
    pub fn all() -> &'static [Technique] {
        &[
            Technique::BackgroundSkim,
            Technique::ForegroundSkim,
            Technique::Extend,
            Technique::ContigSkim,
            Technique::DoneBySize,
            Technique::FindAHome,
            Technique::CannotMerge,
            Technique::LongGetLonger,
        ]
    }

    pub fn name(self) -> &'static str {
        match self {
            Technique::BackgroundSkim => "bg-skim",
            Technique::ForegroundSkim => "fg-skim",
            Technique::Extend => "extend",
            Technique::ContigSkim => "contig-skim",
            Technique::DoneBySize => "done-by-size",
            Technique::FindAHome => "find-a-home",
            Technique::CannotMerge => "cannot-merge",
            Technique::LongGetLonger => "long-get-longer",
        }
    }

    pub fn apply<C: Clue>(self, clues: &[C], lane: &mut [Cell]) -> anyhow::Result<ScrubReport> {
        match self {
            Technique::BackgroundSkim => background_skim_line(clues, lane),
            Technique::ForegroundSkim => foreground_skim_line(clues, lane),
            Technique::Extend => extend_line(clues, lane),
            Technique::ContigSkim => contig_skim_line(clues, lane),
            Technique::DoneBySize => done_by_size_line(clues, lane),
            Technique::FindAHome => find_a_home_line(clues, lane),
            Technique::CannotMerge => cannot_merge_line(clues, lane),
            Technique::LongGetLonger => long_get_longer_line(clues, lane),
        }
    }
}

/// Skims a copy of the lane in which `view` has hidden some of what's known, and learns whatever
/// that turns up. Hiding knowledge only ever makes a lane more permissive, so this can't learn
/// anything that isn't true of the real lane.
fn skim_through<C: Clue>(
    clues: &[C],
    lane: &mut [Cell],
    view: impl Fn(usize, Cell) -> Cell,
) -> anyhow::Result<ScrubReport> {
    let mut viewed: Vec<Cell> = lane
        .iter()
        .enumerate()
        .map(|(i, cell)| view(i, *cell))
        .collect();
    skim_line(clues, &mut viewed)?;

    let mut affected_cells = vec![];
    for (i, learned) in viewed.into_iter().enumerate() {
        learn_cell_intersect(learned, lane, i, &mut affected_cells)?;
    }
    Ok(ScrubReport { affected_cells })
}

/// A skim that only pays attention to known background: the clues get slammed back and forth
/// between the crossed-out cells, without noticing which cells are already painted.
pub fn background_skim_line<C: Clue>(
    clues: &[C],
    lane: &mut [Cell],
) -> anyhow::Result<ScrubReport> {
    skim_through(clues, lane, |_, cell| {
        if cell.is_known_to_be(BACKGROUND) {
            cell
        } else {
            Cell::new_anything()
        }
    })
}

/// A skim that only pays attention to known foreground: the clues get slammed back and forth
/// around the painted cells, without noticing which cells are crossed out.
pub fn foreground_skim_line<C: Clue>(
    clues: &[C],
    lane: &mut [Cell],
) -> anyhow::Result<ScrubReport> {
    skim_through(clues, lane, |_, cell| {
        if cell.can_be(BACKGROUND) {
            Cell::new_anything()
        } else {
            cell
        }
    })
}

/// A painted cell near the edge has to belong to the first clue, so the clue reaches at least as
/// far from the edge as its own length. With a 4 clue, `?#???` becomes `?###?`.
///
/// "The edge" is wherever the unsolved part of the lane starts: cells that are solved all the way
/// from the real edge, up to the last background one, don't count, and neither do the clues
/// they've used up. So `xx#?????` becomes `xx####??`.
///
/// It's deliberately no smarter than that. In particular, it doesn't notice background that's in
/// the clue's way.
pub fn extend_line<C: Clue>(clues: &[C], lane: &mut [Cell]) -> anyhow::Result<ScrubReport> {
    let mut affected_cells = vec![];
    extend_from_edge(clues, lane, false, &mut affected_cells)?;
    extend_from_edge(clues, lane, true, &mut affected_cells)?;
    Ok(ScrubReport { affected_cells })
}

fn extend_from_edge<C: Clue>(
    clues: &[C],
    lane: &mut [Cell],
    reversed: bool,
    affected: &mut Vec<usize>,
) -> anyhow::Result<()> {
    // Everything below counts from the edge we're working from.
    let len = lane.len();
    let lane_idx = |idx: usize| -> usize { if reversed { len - 1 - idx } else { idx } };
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

    // -- Find the edge of the unsolved part, and the first clue that isn't used up --

    let mut start = 0;
    let mut first_clue_idx = 0;

    let mut clue_idx = 0;
    let mut idx_in_clue = None; // in between clues
    for idx in 0..len {
        let Some(color) = lane[lane_idx(idx)].known_or() else {
            break;
        };
        if color == BACKGROUND {
            if idx_in_clue.is_some() {
                bail!("background in the middle of a clue");
            }
            start = idx + 1;
            first_clue_idx = clue_idx;
            continue;
        }
        if clue_idx >= clues.len() {
            bail!("foreground colors after last clue");
        }

        let clue = clue_at(clue_idx);
        let really_idx_in_clue = idx_in_clue.unwrap_or(0);
        if color != clue_color_at(clue, really_idx_in_clue) {
            bail!("incorrect color");
        }
        if really_idx_in_clue + 1 >= clue.len() {
            idx_in_clue = None;
            clue_idx += 1;
        } else {
            idx_in_clue = Some(really_idx_in_clue + 1);
        }
    }

    if first_clue_idx >= clues.len() {
        return Ok(()); // Not our job to fill in the leftover background.
    }
    let clue = clue_at(first_clue_idx);
    if start + clue.len() > len {
        if start == len {
            bail!("some clues unused");
        }
        bail!("clue {clue:?} doesn't fit in the rest of the lane");
    }

    // -- Is there a painted cell close enough to the edge? --

    let Some(first_painted) =
        (0..clue.len()).find(|offset| !lane[lane_idx(start + offset)].can_be(BACKGROUND))
    else {
        return Ok(());
    };

    // The clue covers `first_painted`, and it can't start before the edge, so it starts
    // somewhere in `0..=first_painted`. Wherever that is, it covers the cells from
    // `first_painted` out to its own length.
    for offset in first_painted..clue.len() {
        let mut clue_cell = Cell::new_impossible();
        for clue_start in 0..=first_painted {
            clue_cell.actually_could_be(clue_color_at(clue, offset - clue_start));
        }
        learn_cell_intersect(clue_cell, lane, lane_idx(start + offset), affected)?;
    }

    Ok(())
}

/// A skim that only pays attention to what's been solved all the way out from an edge. Anything
/// past the first unsolved cell (from either end) might as well not be there.
pub fn contig_skim_line<C: Clue>(clues: &[C], lane: &mut [Cell]) -> anyhow::Result<ScrubReport> {
    let from_start = lane.iter().take_while(|cell| cell.is_known()).count();
    let from_end = lane.iter().rev().take_while(|cell| cell.is_known()).count();
    let len = lane.len();

    skim_through(clues, lane, |i, cell| {
        if i < from_start || i + from_end >= len {
            cell
        } else {
            Cell::new_anything()
        }
    })
}

/// Not a technique, because it's not worth the name: once every cell of a color that the clues
/// call for has been painted, nothing else is that color. (For a black-and-white lane, that means
/// that the rest of it is background.)
pub fn rule_out_finished_colors<C: Clue>(
    clues: &[C],
    lane: &mut [Cell],
) -> anyhow::Result<ScrubReport> {
    let mut affected_cells = vec![];

    let mut called_for = [0_usize; u32::BITS as usize];
    for clue in clues {
        for i in 0..clue.len() {
            called_for[clue.color_at(i).0 as usize] += 1;
        }
    }
    let in_play = lane.iter().fold(0, |colors, cell| colors | cell.raw());

    // Ruling out one color can settle some cells, and finish another one off.
    loop {
        let learned_before = affected_cells.len();
        for color in
            Cell::without(&Cell::new_anything(), Cell::from_color(BACKGROUND)).can_be_iter()
        {
            if in_play & Cell::from_color(color).raw() == 0 {
                continue;
            }
            let painted = lane
                .iter()
                .filter(|cell| cell.is_known_to_be(color))
                .count();
            if painted > called_for[color.0 as usize] {
                bail!("too much of color {}", color.0);
            }
            if painted == called_for[color.0 as usize] {
                for i in 0..lane.len() {
                    if !lane[i].is_known_to_be(color) {
                        learn_cell_not(color, lane, i, &mut affected_cells)?;
                    }
                }
            }
        }
        if affected_cells.len() == learned_before {
            return Ok(ScrubReport { affected_cells });
        }
    }
}

// ---- Techniques that count blocks ----
//
// These all need to know which clues are still up in the air. A clue is "fixed" when
// `skim_to_find_fixed_clues` says so, which is what crosses it off in the GUI: skimming can tell
// exactly where it goes, and it's all painted.

/// A stretch of one color in a clue that isn't fixed. Usually, that's the whole clue, but clues
/// with caps have a segment for each cap.
struct Segment {
    color: Color,
    len: usize,
    /// What the cells just outside it could be, according to the rest of the clues.
    before: Cell,
    after: Cell,
}

struct Unfixed {
    /// Which cells of the lane belong to fixed clues.
    fixed_cells: Vec<bool>,
    /// In the order they appear in the lane.
    segments: Vec<Segment>,
}

impl Unfixed {
    /// `None` if the clues are too strange to reason about in terms of blocks: two segments of
    /// the same color could touch, so a block of that color needn't be just one of them.
    fn find<C: Clue>(clues: &[C], lane: &[Cell]) -> Option<Unfixed> {
        if clues.iter().any(|clue| clue.len() == 0) {
            return None;
        }
        let touches_next = |i: usize| -> Option<Color> {
            if i + 1 < clues.len() && !clues[i].must_be_separated_from(&clues[i + 1]) {
                Some(clues[i + 1].color_at(0))
            } else {
                None
            }
        };
        let touches_prev = |i: usize| -> Option<Color> {
            if i > 0 && !clues[i - 1].must_be_separated_from(&clues[i]) {
                Some(clues[i - 1].color_at(clues[i - 1].len() - 1))
            } else {
                None
            }
        };
        for (i, clue) in clues.iter().enumerate() {
            if touches_next(i) == Some(clue.color_at(clue.len() - 1)) {
                return None;
            }
        }

        let mut fixed_cells = vec![false; lane.len()];
        let mut segments = vec![];
        for (i, (clue, fixed_at)) in clues
            .iter()
            .zip(skim_to_place_fixed_clues(clues, lane))
            .enumerate()
        {
            if let Some(start) = fixed_at {
                fixed_cells[start..start + clue.len()].fill(true);
                continue;
            }
            let ranges = clue.color_ranges();
            for (range_idx, (color, range)) in ranges.iter().enumerate() {
                let mut before = Cell::from_color(BACKGROUND);
                let mut after = Cell::from_color(BACKGROUND);
                if range_idx > 0 {
                    before = Cell::from_color(ranges[range_idx - 1].0);
                } else if let Some(color) = touches_prev(i) {
                    before.actually_could_be(color);
                }
                if range_idx + 1 < ranges.len() {
                    after = Cell::from_color(ranges[range_idx + 1].0);
                } else if let Some(color) = touches_next(i) {
                    after.actually_could_be(color);
                }
                segments.push(Segment {
                    color: *color,
                    len: range.len(),
                    before,
                    after,
                });
            }
        }

        Some(Unfixed {
            fixed_cells,
            segments,
        })
    }

    fn colors(&self) -> Vec<Color> {
        let mut res: Vec<Color> = self.segments.iter().map(|segment| segment.color).collect();
        res.sort();
        res.dedup();
        res
    }

    /// The longest segments of a color: how long that is, and which they are.
    fn longest(&self, color: Color) -> (usize, Vec<&Segment>) {
        let of_color = || self.segments.iter().filter(move |s| s.color == color);
        let len = of_color().map(|segment| segment.len).max().unwrap_or(0);
        (len, of_color().filter(|s| s.len == len).collect())
    }

    /// Maximal runs of cells that satisfy `wanted` and don't belong to a fixed clue.
    fn runs(&self, lane: &[Cell], wanted: impl Fn(Cell) -> bool) -> Vec<Range<usize>> {
        let mut res: Vec<Range<usize>> = vec![];
        for (i, cell) in lane.iter().enumerate() {
            if self.fixed_cells[i] || !wanted(*cell) {
                continue;
            }
            match res.last_mut() {
                Some(run) if run.end == i => run.end += 1,
                _ => res.push(i..i + 1),
            }
        }
        res
    }
}

/// A block that's as long as the longest clue that isn't fixed can't get any longer, so it gets
/// background on both sides. (Or, when clues of different colors are involved, it gets whatever
/// the clues that it might be could have next to them.)
///
/// That can be enough to fix some clues, which makes shorter ones the longest, so this repeats
/// until it stops getting anywhere.
pub fn done_by_size_line<C: Clue>(clues: &[C], lane: &mut [Cell]) -> anyhow::Result<ScrubReport> {
    let mut affected_cells = vec![];
    loop {
        let learned_before = affected_cells.len();
        let Some(unfixed) = Unfixed::find(clues, lane) else {
            break;
        };

        for color in unfixed.colors() {
            let (longest, candidates) = unfixed.longest(color);
            let blocks = unfixed.runs(lane, |cell| cell.is_known_to_be(color));
            if blocks.iter().any(|block| block.len() > longest) {
                bail!("a block is longer than any clue that's left");
            }
            let done: Vec<&Range<usize>> = blocks
                .iter()
                .filter(|block| block.len() == longest)
                .collect();
            if done.len() > candidates.len() {
                bail!("more full-length blocks than clues");
            }

            // The blocks are in the same order as the clues they belong to, so each one has
            // only as many candidates as there are blocks still missing (plus one).
            let missing = candidates.len() - done.len();
            for (i, block) in done.iter().enumerate() {
                let mut before = Cell::new_impossible();
                let mut after = Cell::new_impossible();
                for candidate in &candidates[i..=i + missing] {
                    for color in candidate.before.can_be_iter() {
                        before.actually_could_be(color);
                    }
                    for color in candidate.after.can_be_iter() {
                        after.actually_could_be(color);
                    }
                }
                if block.start > 0 {
                    learn_cell_intersect(before, lane, block.start - 1, &mut affected_cells)?;
                }
                if block.end < lane.len() {
                    learn_cell_intersect(after, lane, block.end, &mut affected_cells)?;
                }
            }
        }

        if affected_cells.len() == learned_before {
            break;
        }
    }
    Ok(ScrubReport { affected_cells })
}

/// Looks for places that the longest clues that aren't fixed could go. If there are only just
/// enough of those to go around, each one definitely has a clue in it, and the cells in the
/// middle of it get painted (like a skim of just that part of the lane).
///
/// A place that's long enough for two of them counts twice, and so on.
pub fn find_a_home_line<C: Clue>(clues: &[C], lane: &mut [Cell]) -> anyhow::Result<ScrubReport> {
    let mut affected_cells = vec![];
    let Some(unfixed) = Unfixed::find(clues, lane) else {
        return Ok(ScrubReport { affected_cells });
    };

    for color in unfixed.colors() {
        let (longest, candidates) = unfixed.longest(color);
        // Segments of the same color always have something between them.
        let capacity = |home: &Range<usize>| (home.len() + 1) / (longest + 1);

        let homes = unfixed.runs(lane, |cell| cell.can_be(color));
        let room: usize = homes.iter().map(capacity).sum();
        if room < candidates.len() {
            bail!("nowhere to put a clue");
        }
        if room > candidates.len() {
            continue;
        }

        for home in homes {
            let residents = capacity(&home);
            for i in 0..residents {
                // Slam them all one way, then the other.
                let leftmost_start = home.start + i * (longest + 1);
                let rightmost_start = home.end + 1 - (residents - i) * (longest + 1);
                for idx in rightmost_start..leftmost_start + longest {
                    learn_cell(color, lane, idx, &mut affected_cells)?;
                }
            }
        }
    }

    Ok(ScrubReport { affected_cells })
}

/// An unknown cell between two cells of the same color would join them into one block. If
/// that's longer than any clue that isn't fixed, it isn't that color.
pub fn cannot_merge_line<C: Clue>(clues: &[C], lane: &mut [Cell]) -> anyhow::Result<ScrubReport> {
    let mut affected_cells = vec![];
    let Some(unfixed) = Unfixed::find(clues, lane) else {
        return Ok(ScrubReport { affected_cells });
    };

    for idx in 1..lane.len().saturating_sub(1) {
        let (Some(color), Some(other_color)) = (lane[idx - 1].known_or(), lane[idx + 1].known_or())
        else {
            continue;
        };
        if color != other_color || color == BACKGROUND || !lane[idx].unknown_but_can_be(color) {
            continue;
        }

        let block_before = (0..idx)
            .rev()
            .take_while(|i| lane[*i].is_known_to_be(color))
            .count();
        let block_after = (idx + 1..lane.len())
            .take_while(|i| lane[*i].is_known_to_be(color))
            .count();

        // A fixed clue is as long as it's going to get.
        let too_long = unfixed.fixed_cells[idx - 1]
            || unfixed.fixed_cells[idx + 1]
            || block_before + 1 + block_after > unfixed.longest(color).0;
        if too_long {
            learn_cell_not(color, lane, idx, &mut affected_cells)?;
        }
    }

    Ok(ScrubReport { affected_cells })
}

/// A block that isn't the length of any clue that isn't fixed has to grow, at least to the next
/// size up. It only has as much room as there is between the nearest cells on each side that
/// can't be its color, so if one of those is too close, it has to grow away from it. (With clues
/// 1 and 5, `??x?##????` becomes `??x?####??`.)
pub fn long_get_longer_line<C: Clue>(
    clues: &[C],
    lane: &mut [Cell],
) -> anyhow::Result<ScrubReport> {
    let mut affected_cells = vec![];
    let Some(unfixed) = Unfixed::find(clues, lane) else {
        return Ok(ScrubReport { affected_cells });
    };

    for color in unfixed.colors() {
        let rooms = unfixed.runs(lane, |cell| cell.can_be(color));

        for block in unfixed.runs(lane, |cell| cell.is_known_to_be(color)) {
            let Some(next_size_up) = unfixed
                .segments
                .iter()
                .filter(|segment| segment.color == color && segment.len >= block.len())
                .map(|segment| segment.len)
                .min()
            else {
                bail!("a block is longer than any clue that's left");
            };
            // (A block is made of cells that can be its color, so it's in one of these.)
            let room = rooms
                .iter()
                .find(|room| room.start <= block.start && block.end <= room.end)
                .unwrap();
            if room.len() < next_size_up {
                bail!("a block doesn't have room to grow");
            }

            for idx in block.end..room.start + next_size_up {
                learn_cell(color, lane, idx, &mut affected_cells)?;
            }
            for idx in room.end - next_size_up..block.start {
                learn_cell(color, lane, idx, &mut affected_cells)?;
            }
        }
    }

    Ok(ScrubReport { affected_cells })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::puzzle::{Nono, Triano};

    /// Colors are `x` (background), `#`, and `r`; `?` could be any of those.
    fn parse_color(c: char) -> Color {
        match c {
            'x' => Color(0),
            '#' => Color(1),
            'r' => Color(2),
            _ => panic!("unknown color: {}", c),
        }
    }

    /// Like `l`, but `?` is only background or `#`: what a black-and-white lane looks like once
    /// anything has skimmed it.
    fn bw(spec: &str) -> Vec<Cell> {
        let mut unknown = Cell::from_color(Color(0));
        unknown.actually_could_be(Color(1));
        spec.chars()
            .map(|c| match c {
                '?' => unknown,
                c => Cell::from_color(parse_color(c)),
            })
            .collect()
    }

    fn l(spec: &str) -> Vec<Cell> {
        let mut unknown = bw("?")[0];
        unknown.actually_could_be(Color(2));
        spec.chars()
            .map(|c| match c {
                '?' => unknown,
                c => Cell::from_color(parse_color(c)),
            })
            .collect()
    }

    /// `"#4 r2"` is a black 4, then a red 2.
    fn n(spec: &str) -> Vec<Nono> {
        spec.split_whitespace()
            .map(|chunk| Nono {
                color: parse_color(chunk.chars().next().unwrap()),
                count: chunk[1..].parse().unwrap(),
            })
            .collect()
    }

    fn test<C: Clue>(technique: Technique, clues: Vec<C>, init: Vec<Cell>) -> Vec<Cell> {
        let mut working_line = init;
        technique.apply(&clues, &mut working_line).unwrap();
        working_line
    }

    fn test_bg(clues: &str, init: &str) -> Vec<Cell> {
        test(Technique::BackgroundSkim, n(clues), bw(init))
    }

    fn test_fg(clues: &str, init: &str) -> Vec<Cell> {
        test(Technique::ForegroundSkim, n(clues), bw(init))
    }

    fn test_extend(clues: &str, init: &str) -> Vec<Cell> {
        test(Technique::Extend, n(clues), l(init))
    }

    #[test]
    fn background_skim_test() {
        // With nothing known, it's just a skim:
        assert_eq!(test_bg("#3", "????"), bw("?##?"));
        assert_eq!(test_bg("#1 #2", "????"), bw("#x##"));

        // Background squeezes the clues:
        assert_eq!(test_bg("#3", "x?????"), bw("x??#??"));
        assert_eq!(test_bg("#3", "??x?????"), bw("xxx??#??"));
        assert_eq!(test_bg("#2 #2", "??x???"), bw("##x?#?"));

        // But it doesn't see foreground. (A real skim gets "?##?x" and "xx#x" from these.)
        assert_eq!(test_bg("#3", "?#???"), bw("?##??"));
        assert_eq!(test_bg("#1", "??#?"), bw("??#?"));
    }

    #[test]
    fn foreground_skim_test() {
        // With nothing known, it's just a skim:
        assert_eq!(test_fg("#3", "????"), bw("?##?"));
        assert_eq!(test_fg("#1 #2", "????"), bw("#x##"));

        // Foreground pins the clues down:
        assert_eq!(test_fg("#1", "??#?"), bw("xx#x"));
        assert_eq!(test_fg("#3", "?#???"), bw("?##?x"));
        assert_eq!(test_fg("#2", "?????##?"), bw("xxxxx##x"));

        // But it doesn't see background. (A real skim gets "x??#??" and "##x?#?" from these.)
        assert_eq!(test_fg("#3", "x?????"), bw("x?????"));
        assert_eq!(test_fg("#2 #2", "??x???"), bw("?#x?#?"));
    }

    #[test]
    fn skims_and_what_they_ignore() {
        // What a skim learns can contradict what it ignored...
        assert!(foreground_skim_line(&n("#4"), &mut bw("??x???")).is_err());
        // ...but it doesn't go looking for trouble there.
        assert!(background_skim_line(&n("#1"), &mut bw("#?#")).is_ok());
    }

    #[test]
    fn contig_skim_test() {
        // What's solved out from the edge counts:
        assert_eq!(
            test(Technique::ContigSkim, n("#4"), bw("xx#?????")),
            bw("xx####xx")
        );
        assert_eq!(
            test(Technique::ContigSkim, n("#1 #2"), bw("??????##")),
            bw("?????x##")
        );

        // Nothing else does. (A real skim gets "xx?###xx", "xx#xxx##", and "xx#x" from these.)
        assert_eq!(
            test(Technique::ContigSkim, n("#4"), bw("xx?#????")),
            bw("xx?###??")
        );
        assert_eq!(
            test(Technique::ContigSkim, n("#1 #2"), bw("??#???##")),
            bw("??#??x##")
        );
        assert_eq!(test(Technique::ContigSkim, n("#1"), bw("??#?")), bw("??#?"));
    }

    #[test]
    fn rule_out_finished_colors_test() {
        let finish = |clues: &str, init: Vec<Cell>| {
            let mut working_line = init;
            rule_out_finished_colors(&n(clues), &mut working_line).map(|_| working_line)
        };

        assert_eq!(finish("#3", bw("?###??")).unwrap(), bw("x###xx"));
        assert_eq!(finish("#2 #1", bw("##???#")).unwrap(), bw("##xxx#"));
        assert_eq!(finish("", bw("???")).unwrap(), bw("xxx"));
        // Colors that were never going to turn up are finished from the start:
        assert_eq!(finish("#2", l("?#??")).unwrap(), bw("?#??"));

        // There's still some to find:
        assert_eq!(finish("#3", bw("?##???")).unwrap(), bw("?##???"));

        assert!(finish("#1", bw("#?#")).is_err());

        // Finishing one color can finish another:
        let mut lane = l("r??");
        lane[1].learn_that_not(BACKGROUND).unwrap();
        assert_eq!(finish("r1 #1", lane).unwrap(), l("r#x"));
    }

    #[test]
    fn done_by_size_test() {
        assert_eq!(
            test(Technique::DoneBySize, n("#3 #1"), bw("??###?????")),
            bw("?x###x????")
        );
        // It doesn't take all of the longest clues:
        assert_eq!(
            test(Technique::DoneBySize, n("#3 #3"), bw("???###??????")),
            bw("??x###x?????")
        );
        // A clue that's already fixed is none of its business, though. (There's only one place
        // that the first 3 could be.)
        assert_eq!(
            test(Technique::DoneBySize, n("#3 #3"), bw("?###????")),
            bw("?###????")
        );

        // Shorter than the longest clue, so it might not be done:
        assert_eq!(
            test(Technique::DoneBySize, n("#3 #1"), bw("??#?????")),
            bw("??#?????")
        );
        assert_eq!(
            test(Technique::DoneBySize, n("#3 #2 #2"), bw("?##???##??????")),
            bw("?##???##??????")
        );
        // ...unless the longer clues are out of the picture. (Skimming pins the 3 down.)
        assert_eq!(
            test(Technique::DoneBySize, n("#3 #2 #2"), bw("?###???##?????")),
            bw("?###??x##x????")
        );
        // Finishing off the 3 is what fixes it here, which makes 1 the longest clue left:
        assert_eq!(
            test(Technique::DoneBySize, n("#3 #1 #1"), bw("??###????#????")),
            bw("?x###x??x#x???")
        );

        assert!(done_by_size_line(&n("#2"), &mut bw("?###?")).is_err());
    }

    #[test]
    fn done_by_size_color_test() {
        // Whichever of the red clues this is, it isn't followed by a red cell...
        let result = test(Technique::DoneBySize, n("r2 #1 r2"), l("???rr????"));
        assert_eq!(result[3..5], l("rr"));
        assert_eq!(result[5], bw("?")[0]);
        // ...or preceded by one.
        assert_eq!(result[2], bw("?")[0]);

        // Caps are blocks, too:
        let capped = Triano {
            front_cap: Some(Color(2)),
            body_color: Color(1),
            body_len: 2,
            back_cap: None,
        };
        assert_eq!(
            test(Technique::DoneBySize, vec![capped], l("???##???")),
            l("?xr##x??")
        );
    }

    #[test]
    fn find_a_home_test() {
        // Only one place is big enough:
        assert_eq!(
            test(Technique::FindAHome, n("#3"), bw("??x????x?")),
            bw("??x?##?x?")
        );
        // There are places for the 1, too, but too many of them to say anything:
        assert_eq!(
            test(Technique::FindAHome, n("#1 #3"), bw("??x????x?")),
            bw("??x?##?x?")
        );

        // Two places, but both clues could be in the first one:
        assert_eq!(
            test(Technique::FindAHome, n("#3 #3"), bw("???????x????")),
            bw("???????x????")
        );
        // If there's only just room for both, they're both there:
        assert_eq!(
            test(Technique::FindAHome, n("#3 #3 #1"), bw("???????x?x")),
            bw("###?###x?x")
        );

        // A fixed clue doesn't need a home, and isn't one:
        assert_eq!(
            test(Technique::FindAHome, n("#4 #3"), bw("x####x?????x??")),
            bw("x####x??#??x??")
        );

        assert!(find_a_home_line(&n("#3"), &mut bw("??x??")).is_err());
    }

    #[test]
    fn find_a_home_color_test() {
        assert_eq!(
            test(Technique::FindAHome, n("r3 #1"), l("??x????x??")),
            l("??x?rr?x??")
        );
        // What matters is where the color could go, not where background couldn't:
        assert_eq!(
            test(Technique::FindAHome, n("#1 r3 #1"), l("??#????#??")),
            l("??#?rr?#??")
        );
    }

    #[test]
    fn cannot_merge_test() {
        assert_eq!(
            test(Technique::CannotMerge, n("#2 #2"), bw("?#?#??")),
            bw("?#x#??")
        );
        assert_eq!(
            test(Technique::CannotMerge, n("#1 #3 #1"), bw("???#?##???")),
            bw("???#x##???")
        );

        // It would fit:
        assert_eq!(
            test(Technique::CannotMerge, n("#3"), bw("?#?#?")),
            bw("?#?#?")
        );
        assert_eq!(
            test(Technique::CannotMerge, n("#1 #4 #1"), bw("???#?##???")),
            bw("???#?##???")
        );

        // The 3 would fit, but the 1 is fixed:
        assert_eq!(
            test(Technique::CannotMerge, n("#1 #3"), bw("x#?#????")),
            bw("x#x#????")
        );

        // Only blocks of the same color merge:
        let result = test(Technique::CannotMerge, n("r2 #2"), l("?r?r??"));
        assert_eq!(result[2], bw("?")[0]);
        assert_eq!(
            test(Technique::CannotMerge, n("r1 #1"), l("?r?#??")),
            l("?r?#??")
        );
    }

    #[test]
    fn long_get_longer_test() {
        // Too long to be a 1, so it's the 5, and it has to grow away from whatever's close:
        assert_eq!(
            test(Technique::LongGetLonger, n("#1 #5"), bw("??x?##????")),
            bw("??x?####??")
        );
        assert_eq!(
            test(
                Technique::LongGetLonger,
                n("#1 #1 #5"),
                bw("??????##?x????")
            ),
            bw("????####?x????")
        );
        // The edge is as good as background...
        assert_eq!(
            test(Technique::LongGetLonger, n("#4 #1"), bw("?##??????")),
            bw("?###?????")
        );
        // ...and so is another color.
        assert_eq!(
            test(Technique::LongGetLonger, n("#3 r1"), l("?##r????")),
            l("###r????")
        );

        // Squeezed from both sides:
        assert_eq!(
            test(Technique::LongGetLonger, n("#1 #5"), bw("??x?##??x?")),
            bw("??x#####x?")
        );

        // It might be the 2, which doesn't need to grow:
        assert_eq!(
            test(Technique::LongGetLonger, n("#2 #5"), bw("??x?##????")),
            bw("??x?##????")
        );
        // Plenty of room:
        assert_eq!(
            test(Technique::LongGetLonger, n("#1 #5"), bw("?????##?????")),
            bw("?????##?????")
        );

        assert!(long_get_longer_line(&n("#1 #4"), &mut bw("x##x????")).is_err());
    }

    #[test]
    fn extend_test() {
        assert_eq!(test_extend("#4", "xx?#?????"), l("xx?###???"));
        assert_eq!(test_extend("#4", "xx#??????"), l("xx####???"));

        // From the other edge, too:
        assert_eq!(test_extend("#4", "?????#?xx"), l("???###?xx"));
        assert_eq!(test_extend("#2 #4", "???????#?"), l("?????###?"));
        // ...and from both at once:
        assert_eq!(test_extend("#3 #3", "?#?????#?"), l("?##???##?"));

        // Too far from the edge to say anything:
        assert_eq!(test_extend("#4", "????#????"), l("????#????"));
        assert_eq!(test_extend("#4", "?x?#?????"), l("?x?#?????"));

        // Clues that are used up are out of the picture:
        assert_eq!(test_extend("#1 #2 #4", "#x##x?#????"), l("#x##x?###??"));
        assert_eq!(test_extend("#1 #2 #4", "#x##?#?????"), l("#x##?#?????"));
        // The solved part ends at its last background cell, so a clue can stick out of it:
        assert_eq!(test_extend("#1 #4", "#x##?????"), l("#x####???"));

        // It doesn't finish off a line (that's not extending anything):
        assert_eq!(test_extend("#1", "#x???"), l("#x???"));
        assert_eq!(test_extend("", "?????"), l("?????"));
        assert_eq!(test_extend("#2", "x##xx"), l("x##xx"));

        // A cell that just can't be background is enough to go on:
        let mut lane = l("??????");
        lane[1].learn_that_not(BACKGROUND).unwrap();
        extend_line(&n("r3"), &mut lane).unwrap();
        assert_eq!(lane[1..3], l("rr"));
    }

    #[test]
    fn extend_color_test() {
        // Only the first clue gets extended, whatever color the painted cell is:
        assert_eq!(test_extend("r3 #2", "?r??????"), l("?rr?????"));
        // Different colors can touch, so the 2 might already be past the first cell:
        assert_eq!(test_extend("r2 #3", "r???????"), l("rr??????"));

        assert!(extend_line(&n("r3 #2"), &mut l("?#??????")).is_err());
    }

    #[test]
    fn extend_tri_test() {
        let capped = Triano {
            front_cap: Some(Color(2)),
            body_color: Color(1),
            body_len: 3,
            back_cap: None,
        };
        // (Cap, then three of body.) The second cell could be the cap or the body, depending on
        // where the clue starts, but the next two are definitely body.
        let mut lane = l("?#??????");
        extend_line(&[capped], &mut lane).unwrap();
        assert_eq!(lane, l("?###????"));

        // From the far edge, the cap is the last thing the clue reaches:
        let mut lane = l("??????#?");
        extend_line(&[capped], &mut lane).unwrap();
        let mut cap_or_body = Cell::from_color(Color(1));
        cap_or_body.actually_could_be(Color(2));
        assert_eq!(lane[5..], l("##?"));
        assert_eq!(lane[4], cap_or_body);
    }

    #[test]
    fn extend_contradictions() {
        assert!(extend_line(&n("#2"), &mut l("#x???")).is_err());
        assert!(extend_line(&n("#1"), &mut l("#x#??")).is_err());
        assert!(extend_line(&n("#1 #4"), &mut l("#x???")).is_err());
        assert!(extend_line(&n("#3"), &mut l("?#x??")).is_err());
    }
}
