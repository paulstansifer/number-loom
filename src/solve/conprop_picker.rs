use std::{
    collections::{BinaryHeap, HashMap},
    ops::{AddAssign, Index, IndexMut},
};

use rand::{Rng, seq::SliceRandom};
use typed_index_collections::TiVec;

use crate::{
    geometry::{CellIdx, GridKind, LaneIdx},
    puzzle::{BACKGROUND, Clue, Color, Palette, Puzzle},
    solve::{grid_solve, line_solve::Cell},
};

type Pick = (CellIdx, Color);

/// What `prob_range` starts as: an empty range, which the first probability widens to a point.
const UNSCORED: (f32, f32) = (f32::INFINITY, f32::NEG_INFINITY);

#[derive(Clone)]
pub struct Picker {
    /// Every pick remaining
    candidates: Vec<Pick>,
    /// Parallel to `candidates`: whether it's been picked already.
    taken: Vec<bool>,
    // Two scoring metrics to alternate between
    by_clue_len: BinaryHeap<(Score, usize)>,
    by_disagreement: BinaryHeap<(Score, usize)>,
    clue_len_turn: bool,
    picks_made: usize,
}

impl Picker {
    pub fn pick<C: Clue, K: GridKind>(
        &mut self,
        puzzle: &Puzzle<C, K>,
        grid: &TiVec<CellIdx, Cell>,
        vsids: &HashMap<Pick, f32>,
    ) -> Option<Pick> {
        self.picks_made += 1;
        // Rescoring seems to have no substantial effect either way, but I suspect it
        // might be beneficial in tough cases.
        if self.picks_made % 5 == 0 {
            self.rescore(puzzle, grid, vsids);
        }

        // Take the best pick not yet taken, alternating between the two rankings.
        loop {
            let ranking = if self.clue_len_turn {
                &mut self.by_clue_len
            } else {
                &mut self.by_disagreement
            };
            // Both rankings hold every candidate, so if one runs dry, everything's been taken.
            let (_, idx) = ranking.pop()?;
            if !self.taken[idx] {
                self.taken[idx] = true;
                self.clue_len_turn = !self.clue_len_turn;
                return Some(self.candidates[idx]);
            }
        }
    }

    /// Add the length of the longest unfixed clue of each color to `lane_clue_len` (BACKGROUND is the max of all the colors)
    /// Extend `prob_range` to include the implied probability that each cell is that color, from this lane's point of view.
    fn score_lane<C: Clue, K: GridKind>(
        lane_idx: LaneIdx,
        clue_line: &[C],
        fixed_clues: &TiVec<LaneIdx, Vec<usize>>,
        puzzle: &Puzzle<C, K>,
        grid: &TiVec<CellIdx, Cell>,
        lane_clue_len: &mut PickTable<usize>,
        prob_range: &mut PickTable<(f32, f32)>,
    ) {
        let mut max_clue = HashMap::new();
        let mut color_count: HashMap<Color, usize> =
            puzzle.palette.keys().map(|&color| (color, 0)).collect();
        for (clue_idx, clue) in clue_line.iter().enumerate() {
            for (color, range) in clue.color_ranges() {
                *color_count.entry(color).or_insert(0) += range.len();

                if !fixed_clues[lane_idx].contains(&clue_idx) {
                    // Only non-fixed clues count for length:
                    let v = max_clue.entry(color).or_insert(0);
                    *v = (*v).max(range.len());
                }
            }
        }

        let cell_line = &puzzle.geometry.lane_map.lanes[lane_idx].cells;

        color_count.insert(
            BACKGROUND,
            cell_line.len() - color_count.values().sum::<usize>(), // it gets whatever's left
        );
        let mut color_denominator = HashMap::new();

        for cell_idx in cell_line {
            for (&color, count) in color_count.iter_mut() {
                if grid[*cell_idx].is_known_to_be(color) {
                    *count -= 1;
                } else if grid[*cell_idx].can_be(color) {
                    // Count possible, but not *known* locations for that color:
                    color_denominator.entry(color).or_insert(0).add_assign(1);
                }
            }
        }

        let mut max_len = 0; // over all colors

        // Each lane contributes its highest unfixed clue.
        for (color, len) in &max_clue {
            for &cell_idx in cell_line {
                lane_clue_len[(cell_idx, *color)] += len;
            }
            max_len = max_len.max(*len);
        }
        for &cell_idx in cell_line {
            lane_clue_len[(cell_idx, BACKGROUND)] += max_len;
        }

        // Each lane extends the range of implied probabilities.
        for (color, cell_count) in &color_count {
            if *color_denominator.entry(*color).or_insert(0) == 0 {
                continue; // ultimately irrelevant, but don't crash.
            }
            let prob = *cell_count as f32 / color_denominator[color] as f32;
            for &cell_idx in cell_line {
                let (lo, hi) = &mut prob_range[(cell_idx, *color)];
                *lo = lo.min(prob); // extend
                *hi = hi.max(prob); // extend
            }
        }
    }

    pub fn from_situation<C: Clue, K: GridKind>(
        puzzle: &Puzzle<C, K>,
        grid: &TiVec<CellIdx, Cell>,
        vsids: &HashMap<Pick, f32>,
        rng: &mut impl Rng,
    ) -> Picker {
        let mut possible_guesses = vec![];

        for (idx, cell) in grid.iter_enumerated() {
            if cell.is_known() {
                continue;
            }
            for color in cell.can_be_iter() {
                possible_guesses.push((idx, color));
            }
        }
        possible_guesses.shuffle(rng);

        let mut res = Picker {
            taken: vec![false; possible_guesses.len()],
            candidates: possible_guesses,
            by_clue_len: BinaryHeap::new(),
            by_disagreement: BinaryHeap::new(),
            clue_len_turn: true,
            picks_made: 0,
        };

        res.rescore(puzzle, grid, vsids);

        res
    }

    fn rescore<C: Clue, K: GridKind>(
        &mut self,
        puzzle: &Puzzle<C, K>,
        grid: &TiVec<CellIdx, Cell>,
        vsids: &HashMap<Pick, f32>,
    ) {
        let mut taken = std::mem::take(&mut self.taken).into_iter();
        self.candidates.retain(|&(cell_idx, color)| {
            !taken.next().unwrap() && grid[cell_idx].unknown_but_can_be(color)
        });
        self.taken = vec![false; self.candidates.len()];

        let fixed_clues = grid_solve::fixed_clues(puzzle, grid);

        let cell_count = puzzle.geometry.cell_count();
        let mut lane_clue_len = PickTable::new(cell_count, &puzzle.palette, 0);
        let mut prob_range = PickTable::new(cell_count, &puzzle.palette, UNSCORED);

        for (lane_idx, clue_line) in puzzle.lines.iter_enumerated() {
            Self::score_lane(
                lane_idx,
                clue_line,
                &fixed_clues,
                puzzle,
                grid,
                &mut lane_clue_len,
                &mut prob_range,
            );
        }

        let rank = |score: &dyn Fn(Pick) -> f32| -> BinaryHeap<(Score, usize)> {
            let scored = self.candidates.iter().enumerate();
            scored
                .map(|(idx, &pick)| (Score(score(pick)), idx))
                .collect()
        };

        // Longest first:
        self.by_clue_len =
            rank(&|pick| lane_clue_len[pick] as f32 + *vsids.get(&pick).unwrap_or(&0.0) * 0.5);

        // Widest separation first:
        self.by_disagreement = rank(&|pick| {
            let (lo, hi) = prob_range[pick];
            hi - lo + vsids.get(&pick).unwrap_or(&0.0) * 0.05
        });

        self.clue_len_turn = true;
    }
}

#[derive(Clone, Copy, PartialEq, PartialOrd)]
struct Score(f32);

impl Eq for Score {}

impl Ord for Score {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.total_cmp(&other.0)
    }
}

/// This is a perhaps somewhat excessive replacement for `HashMap<Pick, T>`
/// that is faster because we know the keys are dense.
/// (`nohash-hasher` would be a good alternative if `Pick` were just one natural number)
struct PickTable<T> {
    /// One past the highest color in the palette (which might have gaps).
    stride: usize,
    data: Vec<T>,
}

impl<T: Clone> PickTable<T> {
    fn new(cell_count: usize, palette: &Palette, fill: T) -> Self {
        let stride = palette.keys().map(|c| c.0 as usize).max().unwrap_or(0) + 1;
        PickTable {
            stride,
            data: vec![fill; cell_count * stride],
        }
    }
}

impl<T> PickTable<T> {
    fn flat_idx(&self, (cell, color): Pick) -> usize {
        // Otherwise, it'd silently alias the next cell's entry.
        debug_assert!(
            (color.0 as usize) < self.stride,
            "{color:?} isn't in the palette"
        );
        cell.0 as usize * self.stride + color.0 as usize
    }
}

impl<T> Index<Pick> for PickTable<T> {
    type Output = T;
    fn index(&self, pick: Pick) -> &T {
        &self.data[self.flat_idx(pick)]
    }
}

impl<T> IndexMut<Pick> for PickTable<T> {
    fn index_mut(&mut self, pick: Pick) -> &mut T {
        let idx = self.flat_idx(pick);
        &mut self.data[idx]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashSet;

    use rand::SeedableRng;

    use crate::{
        geometry::Square,
        puzzle::{ColorInfo, Nono, Triano},
        solve::grid_solve::{SolveContext, SolveOptions, SolveState},
    };

    const C1: Color = Color(1);
    const C2: Color = Color(2);

    fn palette() -> HashMap<Color, ColorInfo> {
        let mut palette = HashMap::new();
        palette.insert(BACKGROUND, ColorInfo::default_bg());
        palette.insert(C1, ColorInfo::default_fg(C1));
        palette.insert(C2, ColorInfo::default_fg(C2));
        palette
    }

    fn nono(color: Color, count: u16) -> Nono {
        Nono { color, count }
    }

    /// `?` could be anything, `.` is known background, `1`/`2` are known colors, and `a` is
    /// "background or 1, but not 2".
    fn grid(cells: &str) -> TiVec<CellIdx, Cell> {
        cells
            .chars()
            .map(|ch| match ch {
                '?' => Cell::new(&palette()),
                '.' => Cell::from_color(BACKGROUND),
                '1' => Cell::from_color(C1),
                '2' => Cell::from_color(C2),
                'a' => {
                    let mut cell = Cell::from_color(BACKGROUND);
                    cell.actually_could_be(C1);
                    cell
                }
                _ => panic!("unknown cell {ch:?}"),
            })
            .collect()
    }

    struct Scores {
        lane_clue_len: PickTable<usize>,
        prob_range: PickTable<(f32, f32)>,
    }

    impl Scores {
        fn new() -> Self {
            Scores {
                // Enough cells for any lane in these tests.
                lane_clue_len: PickTable::new(16, &palette(), 0),
                prob_range: PickTable::new(16, &palette(), UNSCORED),
            }
        }

        /// Score the only lane of a single-lane puzzle, with `clues` and `fixed` as given.
        fn score<C: Clue>(&mut self, clues: Vec<C>, fixed: Vec<usize>, cells: &str) {
            let puzzle = Puzzle::<C, Square>::single_lane(palette(), cells.len(), clues.clone());
            let fixed_clues: TiVec<LaneIdx, Vec<usize>> = vec![fixed].into();
            Picker::score_lane(
                LaneIdx(0),
                &clues,
                &fixed_clues,
                &puzzle,
                &grid(cells),
                &mut self.lane_clue_len,
                &mut self.prob_range,
            );
        }

        fn len(&self, cell: u32, color: Color) -> usize {
            self.lane_clue_len[(CellIdx(cell), color)]
        }

        fn prob(&self, cell: u32, color: Color) -> (f32, f32) {
            self.prob_range[(CellIdx(cell), color)]
        }
    }

    #[test]
    fn blank_lane_one_clue() {
        let mut s = Scores::new();
        s.score(vec![nono(C1, 2)], vec![], "?????");

        for cell in 0..5 {
            assert_eq!(s.len(cell, C1), 2);
            assert_eq!(
                s.len(cell, BACKGROUND),
                2,
                "background takes the max over colors"
            );
            assert_eq!(s.prob(cell, C1), (2.0 / 5.0, 2.0 / 5.0));
            assert_eq!(s.prob(cell, BACKGROUND), (3.0 / 5.0, 3.0 / 5.0));
        }
        // C2 isn't in this lane's clues, so this lane says it can't be here.
        assert_eq!(s.len(0, C2), 0);
        assert_eq!(s.prob(0, C2), (0.0, 0.0));
    }

    #[test]
    fn multiple_colors_take_the_longest_clue_and_sum_the_counts() {
        let mut s = Scores::new();
        s.score(
            vec![nono(C1, 3), nono(C2, 1), nono(C1, 1)],
            vec![],
            "????????",
        );

        assert_eq!(s.len(0, C1), 3);
        assert_eq!(s.len(0, C2), 1);
        assert_eq!(s.len(0, BACKGROUND), 3);
        assert_eq!(s.prob(0, C1), (4.0 / 8.0, 4.0 / 8.0));
        assert_eq!(s.prob(0, C2), (1.0 / 8.0, 1.0 / 8.0));
        assert_eq!(s.prob(0, BACKGROUND), (3.0 / 8.0, 3.0 / 8.0));
    }

    #[test]
    fn empty_clue_line() {
        let mut s = Scores::new();
        s.score(Vec::<Nono>::new(), vec![], "???");

        assert_eq!(s.len(0, BACKGROUND), 0);
        assert_eq!(s.len(0, C1), 0);
        assert_eq!(s.prob(0, BACKGROUND), (1.0, 1.0));
    }

    #[test]
    fn known_cells_leave_the_numerator_and_the_denominator() {
        let mut s = Scores::new();
        // One background cell and one C1 cell are already placed.
        s.score(vec![nono(C1, 2)], vec![], ".1???");

        // C1: 2 cells, 1 already placed, 3 unknown places to put the other.
        assert_eq!(s.prob(2, C1), (1.0 / 3.0, 1.0 / 3.0));
        // Background: 3 cells, 1 already placed, 3 unknown places to put the other 2.
        assert_eq!(s.prob(2, BACKGROUND), (2.0 / 3.0, 2.0 / 3.0));
    }

    #[test]
    fn cells_that_cannot_be_a_color_leave_its_denominator() {
        let mut s = Scores::new();
        // Cells 0 and 1 can't be C2; everything can be C1 or background.
        s.score(vec![nono(C1, 1), nono(C2, 1)], vec![], "aa???");

        assert_eq!(s.prob(2, C1), (1.0 / 5.0, 1.0 / 5.0));
        assert_eq!(s.prob(2, C2), (1.0 / 3.0, 1.0 / 3.0));
        assert_eq!(s.prob(2, BACKGROUND), (3.0 / 5.0, 3.0 / 5.0));
    }

    #[test]
    fn fixed_clues_do_not_count_for_length() {
        let mut s = Scores::new();
        // The 3 is placed; the 1 isn't.
        s.score(vec![nono(C1, 3), nono(C1, 1)], vec![0], "111.????");

        assert_eq!(s.len(5, C1), 1);
        assert_eq!(s.len(5, BACKGROUND), 1);
        // ...but they still count toward how many cells are left.
        assert_eq!(s.prob(5, C1), (1.0 / 4.0, 1.0 / 4.0));
        assert_eq!(s.prob(5, BACKGROUND), (3.0 / 4.0, 3.0 / 4.0));
    }

    #[test]
    fn fixed_clues_from_the_real_fixed_clue_finder() {
        let clues = vec![nono(C1, 3), nono(C1, 1)];
        let cells = "111.????";
        let puzzle = Puzzle::<Nono, Square>::single_lane(palette(), cells.len(), clues.clone());
        let g = grid(cells);
        let fixed_clues = grid_solve::fixed_clues(&puzzle, &g);
        assert_eq!(fixed_clues[LaneIdx(0)], vec![0]);

        let mut s = Scores::new();
        Picker::score_lane(
            LaneIdx(0),
            &clues,
            &fixed_clues,
            &puzzle,
            &g,
            &mut s.lane_clue_len,
            &mut s.prob_range,
        );
        assert_eq!(s.len(5, C1), 1);
    }

    #[test]
    fn a_color_with_only_fixed_clues_gets_no_length() {
        let mut s = Scores::new();
        s.score(vec![nono(C2, 2), nono(C1, 1)], vec![0], "22.???");

        assert_eq!(s.len(4, C2), 0);
        assert_eq!(s.len(4, C1), 1);
        assert_eq!(s.len(4, BACKGROUND), 1);
    }

    #[test]
    fn scores_accumulate_across_lanes() {
        let mut s = Scores::new();
        s.score(vec![nono(C1, 2)], vec![], "????");
        s.score(vec![nono(C1, 3)], vec![], "????");

        // Lengths add up...
        assert_eq!(s.len(0, C1), 5);
        assert_eq!(s.len(0, BACKGROUND), 5);
        // ...and probabilities stretch the range.
        assert_eq!(s.prob(0, C1), (2.0 / 4.0, 3.0 / 4.0));
        assert_eq!(s.prob(0, BACKGROUND), (1.0 / 4.0, 2.0 / 4.0));

        // A lane in the middle of the range doesn't shrink it.
        s.score(vec![nono(C1, 1), nono(C1, 1)], vec![], "????");
        assert_eq!(s.prob(0, C1), (2.0 / 4.0, 3.0 / 4.0));
        assert_eq!(s.len(0, C1), 6);
    }

    #[test]
    fn triano_caps_count_as_their_own_color() {
        let mut s = Scores::new();
        let clue = Triano {
            front_cap: Some(C2),
            body_len: 3,
            body_color: C1,
            back_cap: Some(C2),
        };
        s.score(vec![clue], vec![], "????????");

        assert_eq!(s.len(0, C1), 3);
        assert_eq!(s.len(0, C2), 1, "the two caps are separate runs");
        assert_eq!(s.len(0, BACKGROUND), 3);
        assert_eq!(s.prob(0, C1), (3.0 / 8.0, 3.0 / 8.0));
        assert_eq!(s.prob(0, C2), (2.0 / 8.0, 2.0 / 8.0));
        assert_eq!(s.prob(0, BACKGROUND), (3.0 / 8.0, 3.0 / 8.0));
    }

    #[test]
    fn triano_cap_merging_with_body() {
        let mut s = Scores::new();
        let clue = Triano {
            front_cap: Some(C1),
            body_len: 2,
            body_color: C1,
            back_cap: Some(C2),
        };
        s.score(vec![clue], vec![], "?????");

        assert_eq!(
            s.len(0, C1),
            3,
            "a cap the same color as the body extends it"
        );
        assert_eq!(s.len(0, C2), 1);
    }

    #[test]
    fn solved_lane() {
        let mut s = Scores::new();
        s.score(vec![nono(C1, 2)], vec![0], ".11.");

        assert_eq!(s.len(0, BACKGROUND), 0);
    }

    #[test]
    fn color_fully_placed_but_lane_not_solved() {
        let mut s = Scores::new();
        // C1 is done, but the cells around it could still (in principle) be C1 or background.
        s.score(vec![nono(C1, 2)], vec![0], "a11a");

        assert_eq!(s.prob(0, C1), (0.0, 0.0));
    }

    #[test]
    fn background_fully_placed_but_lane_not_solved() {
        let mut s = Scores::new();
        s.score(vec![nono(C1, 2)], vec![], "..a1");

        assert_eq!(s.prob(2, C1), (1.0, 1.0));
    }

    /// Every (unknown cell, color it could be) pair in `grid`, which is exactly what the order
    /// of picks should be a permutation of.
    fn open_picks(grid: &TiVec<CellIdx, Cell>) -> HashSet<Pick> {
        grid.iter_enumerated()
            .filter(|(_, cell)| !cell.is_known())
            .flat_map(|(idx, cell)| cell.can_be_iter().map(move |color| (idx, color)))
            .collect()
    }

    /// Run `from_situation` on a state for `puzzle` holding `grid` (blank if `None`), after
    /// line-solving if `line_solve`. Returns every pick it would make, in order, and the grid it
    /// looked at.
    fn pick<C: Clue>(
        puzzle: &Puzzle<C, Square>,
        grid: Option<TiVec<CellIdx, Cell>>,
        line_solve: bool,
    ) -> (Vec<Pick>, TiVec<CellIdx, Cell>) {
        let options = SolveOptions::default();
        let mut line_cache = None;
        let mut ctx = SolveContext::new(puzzle, &mut line_cache, &options);
        let grid = grid.unwrap_or_else(|| {
            vec![Cell::new(&puzzle.palette); puzzle.geometry.cell_count()].into()
        });
        let mut ll_state = SolveState::new(&mut ctx, grid);
        if line_solve {
            ll_state.run(&mut ctx).unwrap();
        }
        let mut rng = rand::rngs::StdRng::seed_from_u64(0);
        let mut picker = Picker::from_situation(puzzle, &ll_state.grid, &HashMap::new(), &mut rng);
        let order =
            std::iter::from_fn(|| picker.pick(puzzle, &ll_state.grid, &HashMap::new())).collect();
        (order, ll_state.grid.clone())
    }

    fn bw_palette() -> HashMap<Color, ColorInfo> {
        let mut bw = palette();
        bw.remove(&C2);
        bw
    }

    fn assert_permutation_of_open_picks(order: &[Pick], grid: &TiVec<CellIdx, Cell>) {
        let expected = open_picks(grid);
        let seen: HashSet<Pick> = order.iter().copied().collect();
        assert_eq!(seen.len(), order.len(), "duplicate picks: {order:?}");
        assert_eq!(seen, expected);
    }

    #[test]
    fn order_is_every_open_pick_once() {
        // Two solutions (the diagonals), so line solving can't get anywhere.
        let one = vec![nono(C1, 1)];
        let puzzle = Puzzle::square(bw_palette(), vec![one.clone(); 2], vec![one; 2]);

        for line_solve in [false, true] {
            let (order, grid) = pick(&puzzle, None, line_solve);
            assert_eq!(order.len(), 4 * 2, "four cells, two colors each");
            assert_permutation_of_open_picks(&order, &grid);
        }
    }

    /// The order alternates: the pick with the most clue length through it, then the pick with
    /// the widest range of probabilities, then the next-most clue length, and so on.
    #[test]
    fn order_alternates_between_clue_length_and_disagreement() {
        // Four wide and three tall, from this solution:
        //   ###.
        //   #...
        //   ....
        let puzzle = Puzzle::square(
            bw_palette(),
            vec![vec![nono(C1, 3)], vec![nono(C1, 1)], vec![]],
            vec![
                vec![nono(C1, 2)],
                vec![nono(C1, 1)],
                vec![nono(C1, 1)],
                vec![],
            ],
        );
        let at = |x: usize, y: usize| puzzle.geometry.cell((x, y)).unwrap();

        // Blank the empty lanes (as line solving would), but leave the rest open, so that
        // there's something to disagree about:
        //   ???.
        //   ???.
        //   ....
        let mut grid: TiVec<CellIdx, Cell> =
            vec![Cell::new(&puzzle.palette); puzzle.geometry.cell_count()].into();
        for x in 0..4 {
            grid[at(x, 2)] = Cell::from_color(BACKGROUND);
        }
        for y in 0..3 {
            grid[at(3, y)] = Cell::from_color(BACKGROUND);
        }

        // Most clue length: (0, 0), with 3 across and 2 down; 5 for both colors.
        // Widest disagreement: (0, 1). Its row says 1 is 1/3 likely; its column says certain.
        let (picks, grid) = pick(&puzzle, Some(grid), false);
        assert_permutation_of_open_picks(&picks, &grid);

        let cells: Vec<CellIdx> = picks.iter().map(|&(cell, _)| cell).collect();
        assert_eq!(cells[0..4], [at(0, 0), at(0, 1), at(0, 0), at(0, 1)]);

        // Each cell's two colors tie, so they come out back to back (in either order).
        let colors = |i: usize, j: usize| HashSet::from([picks[i].1, picks[j].1]);
        assert_eq!(colors(0, 2), HashSet::from([BACKGROUND, C1]));
        assert_eq!(colors(1, 3), HashSet::from([BACKGROUND, C1]));
    }

    /// Row 0 has a 2 in it, but column 0 doesn't, so neither lane through cell (0, 0) has
    /// anything to say about it being 2 -- but line solving rules that out, so it's never asked.
    #[test]
    fn colors_ruled_out_by_line_solving_are_not_picked() {
        // Two solutions:
        //   1.2      .12
        //   .1.  or  1..
        let puzzle = Puzzle::square(
            palette(),
            vec![vec![nono(C1, 1), nono(C2, 1)], vec![nono(C1, 1)]],
            vec![vec![nono(C1, 1)], vec![nono(C1, 1)], vec![nono(C2, 1)]],
        );
        let at = |x: usize, y: usize| puzzle.geometry.cell((x, y)).unwrap();

        let (order, grid) = pick(&puzzle, None, true);

        // Line solving should have pinned down column 2 and nothing else.
        assert!(grid[at(2, 0)].is_known_to_be(C2));
        assert!(grid[at(2, 1)].is_known_to_be(BACKGROUND));
        assert!(!grid[at(0, 0)].can_be(C2));

        assert_permutation_of_open_picks(&order, &grid);
        assert_eq!(order.len(), 4 * 2);
    }

    #[test]
    fn solved_puzzle_has_nothing_to_pick() {
        let puzzle = Puzzle::square(
            palette(),
            vec![vec![nono(C1, 1), nono(C2, 1)]],
            vec![vec![nono(C1, 1)], vec![nono(C2, 1)]],
        );

        let (order, grid) = pick(&puzzle, None, true);
        assert!(grid.iter().all(Cell::is_known));
        assert!(order.is_empty());
    }
}
