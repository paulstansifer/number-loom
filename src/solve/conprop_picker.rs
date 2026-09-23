use std::{
    collections::{HashMap, HashSet},
    ops::AddAssign,
};

use typed_index_collections::TiVec;

use crate::{
    geometry::{CellIdx, GridKind},
    gui::LaneIdx,
    puzzle::{BACKGROUND, Clue, Color, Puzzle},
    solve::{conprop::ConpropState, grid_solve, line_solve::Cell},
};

type Pick = (CellIdx, Color);

struct Picker {
    order: Vec<Pick>,
}

impl Picker {
    /// Add the length of the longest unfixed clue of each color to `lane_clue_len` (BACKGROUND is the max of all the colors)
    /// Extend `prob_range` to inclue the implied probability that each cell is that color, from this lane's point of view.
    fn score_lane<C: Clue, K: GridKind>(
        lane_idx: LaneIdx,
        clue_line: &[C],
        fixed_clues: &TiVec<LaneIdx, Vec<usize>>,
        puzzle: &Puzzle<C, K>,
        grid: &TiVec<CellIdx, Cell>,
        lane_clue_len: &mut HashMap<Pick, usize>,
        prob_range: &mut HashMap<Pick, (f32, f32)>,
    ) {
        let mut max_clue = HashMap::new();
        let mut color_count: HashMap<Color, usize> = HashMap::new();
        for (clue_idx, clue) in clue_line.iter().enumerate() {
            for (color, range) in clue.color_ranges() {
                *color_count.entry(color).or_insert(0) += range.clone().count();

                if !fixed_clues[lane_idx].contains(&clue_idx) {
                    // Only non-fixed clues count for length:
                    let v = max_clue.entry(color).or_insert(0);
                    *v = (*v).max(range.count());
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

        for cell_idx in cell_line {
            let mut max_len = 0; // over all colors

            // Each lane contributes its highest unfixed clue.
            for (color, len) in &max_clue {
                *lane_clue_len.entry((*cell_idx, *color)).or_insert(0) += len;

                max_len = max_len.max(*len);
            }
            *lane_clue_len.entry((*cell_idx, BACKGROUND)).or_insert(0) += max_len;

            // Each lane extends the range of implied probabilities.
            for (color, cell_count) in &color_count {
                if *color_denominator.entry(*color).or_insert(0) == 0 {
                    continue; // ultimately irrelevant, but don't crash.
                }
                let prob = *cell_count as f32 / color_denominator[color] as f32;
                prob_range
                    .entry((*cell_idx, *color))
                    .and_modify(|&mut (ref mut lo, ref mut hi)| {
                        *lo = lo.min(prob); // extend
                        *hi = hi.max(prob); // extend
                    })
                    .or_insert((prob, prob));
            }
        }
    }

    pub fn from_situation<'p, C: Clue, K: GridKind>(
        puzzle: &Puzzle<C, K>,
        state: &ConpropState<'p, C>,
    ) -> Picker {
        let mut remaining_indices = vec![];

        for (idx, cell) in state.ll_state.grid.iter_enumerated() {
            if cell.is_known() {
                continue;
            }
            for color in cell.can_be_iter() {
                remaining_indices.push((idx, color));
            }
            if !cell.is_known() {}
        }

        use rand::seq::SliceRandom;
        let mut rng = rand::thread_rng(); // TODO: don't recreate this every time, also, seed it
        remaining_indices.shuffle(&mut rng);

        let fixed_clues = grid_solve::fixed_clues(puzzle, &state.ll_state.grid);

        let mut lane_clue_len: HashMap<Pick, usize> = HashMap::default();
        let mut prob_range: HashMap<Pick, (f32, f32)> = HashMap::default();

        for (lane_idx, clue_line) in puzzle.lines.iter_enumerated() {
            Self::score_lane(
                lane_idx,
                clue_line,
                &fixed_clues,
                puzzle,
                &state.ll_state.grid,
                &mut lane_clue_len,
                &mut prob_range,
            );
        }

        let elts_needed = remaining_indices.len();

        let mut ll_remaining = remaining_indices.clone();

        // Sort shortest-first (best-last):
        ll_remaining.sort_unstable_by_key(|key| lane_clue_len[key]);

        let mut disag_remaining = remaining_indices;

        // Sort smallest-separation-first (best-last):
        disag_remaining.sort_unstable_by(|key_a, key_b| {
            let (a, b) = (prob_range[key_a], prob_range[key_b]);
            (a.1 - a.0).total_cmp(&(b.1 - b.0))
        });

        let mut seen: HashSet<Pick> = HashSet::default();

        let mut res = vec![];

        // TODO: might want to try lazy-sorting (and lazy-shuffling?): I suspect that
        // we only rarely more than 10 elements

        while res.len() < elts_needed {
            let list = if res.len() % 2 == 0 {
                &mut ll_remaining
            } else {
                &mut disag_remaining
            };

            let candidate = list.pop().unwrap();

            if !seen.contains(&candidate) {
                res.push(candidate);
                seen.insert(candidate);
            }
        }

        Picker { order: res }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        lane_clue_len: HashMap<Pick, usize>,
        prob_range: HashMap<Pick, (f32, f32)>,
    }

    impl Scores {
        fn new() -> Self {
            Scores {
                lane_clue_len: HashMap::new(),
                prob_range: HashMap::new(),
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
            self.lane_clue_len[&(CellIdx(cell), color)]
        }

        fn prob(&self, cell: u32, color: Color) -> (f32, f32) {
            self.prob_range[&(CellIdx(cell), color)]
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
        // C2 isn't in this lane's clues, so this lane has nothing to say about it.
        assert!(!s.lane_clue_len.contains_key(&(CellIdx(0), C2)));
        assert!(!s.prob_range.contains_key(&(CellIdx(0), C2)));
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
        assert!(!s.lane_clue_len.contains_key(&(CellIdx(0), C1)));
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

        assert!(!s.lane_clue_len.contains_key(&(CellIdx(4), C2)));
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

    /// Every (unknown cell, color it could be) pair in `grid`, which is exactly what `order`
    /// should be a permutation of.
    fn open_picks(grid: &TiVec<CellIdx, Cell>) -> HashSet<Pick> {
        grid.iter_enumerated()
            .filter(|(_, cell)| !cell.is_known())
            .flat_map(|(idx, cell)| cell.can_be_iter().map(move |color| (idx, color)))
            .collect()
    }

    /// Run `from_situation` on a state for `puzzle` holding `grid` (blank if `None`), after
    /// line-solving if `line_solve`. Returns the picker and the grid it looked at.
    fn pick<C: Clue>(
        puzzle: &Puzzle<C, Square>,
        grid: Option<TiVec<CellIdx, Cell>>,
        line_solve: bool,
    ) -> (Picker, TiVec<CellIdx, Cell>) {
        let options = SolveOptions::default();
        let mut line_cache = None;
        let mut ctx = SolveContext::new(puzzle, &mut line_cache, &options);
        let grid = grid.unwrap_or_else(|| {
            vec![Cell::new(&puzzle.palette); puzzle.geometry.cell_count()].into()
        });
        let mut ll_state = SolveState::new(&mut ctx, grid);
        if line_solve {
            ll_state.run_and_check(&mut ctx).unwrap();
        }
        let state = ConpropState::new(ll_state);
        let picker = Picker::from_situation(puzzle, &state);
        (picker, state.ll_state.grid.clone())
    }

    fn bw_palette() -> HashMap<Color, ColorInfo> {
        let mut bw = palette();
        bw.remove(&C2);
        bw
    }

    fn assert_permutation_of_open_picks(picker: &Picker, grid: &TiVec<CellIdx, Cell>) {
        let expected = open_picks(grid);
        let seen: HashSet<Pick> = picker.order.iter().copied().collect();
        assert_eq!(
            seen.len(),
            picker.order.len(),
            "duplicate picks: {:?}",
            picker.order
        );
        assert_eq!(seen, expected);
    }

    #[test]
    fn order_is_every_open_pick_once() {
        // Two solutions (the diagonals), so line solving can't get anywhere.
        let one = vec![nono(C1, 1)];
        let puzzle = Puzzle::square(bw_palette(), vec![one.clone(); 2], vec![one; 2]);

        for line_solve in [false, true] {
            let (picker, grid) = pick(&puzzle, None, line_solve);
            assert_eq!(picker.order.len(), 4 * 2, "four cells, two colors each");
            assert_permutation_of_open_picks(&picker, &grid);
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
        let (picker, grid) = pick(&puzzle, Some(grid), false);
        assert_permutation_of_open_picks(&picker, &grid);

        let cells: Vec<CellIdx> = picker.order.iter().map(|&(cell, _)| cell).collect();
        assert_eq!(cells[0..4], [at(0, 0), at(0, 1), at(0, 0), at(0, 1)]);

        // Each cell's two colors tie, so they come out back to back (in either order).
        let colors = |i: usize, j: usize| HashSet::from([picker.order[i].1, picker.order[j].1]);
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

        let (picker, grid) = pick(&puzzle, None, true);

        // Line solving should have pinned down column 2 and nothing else.
        assert!(grid[at(2, 0)].is_known_to_be(C2));
        assert!(grid[at(2, 1)].is_known_to_be(BACKGROUND));
        assert!(!grid[at(0, 0)].can_be(C2));

        assert_permutation_of_open_picks(&picker, &grid);
        assert_eq!(picker.order.len(), 4 * 2);
    }

    #[test]
    fn solved_puzzle_has_nothing_to_pick() {
        let puzzle = Puzzle::square(
            palette(),
            vec![vec![nono(C1, 1), nono(C2, 1)]],
            vec![vec![nono(C1, 1)], vec![nono(C2, 1)]],
        );

        let (picker, grid) = pick(&puzzle, None, true);
        assert!(grid.iter().all(Cell::is_known));
        assert!(picker.order.is_empty());
    }
}
