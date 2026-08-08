use std::{fmt::Debug, sync::mpsc, vec};

use anyhow::Context;
use colored::Colorize;
use ndarray::{ArrayView1, ArrayViewMut1};

use crate::{
    geometry::{GridKind, LaneMap},
    gui,
    line_solve::{
        Cell, ModeMap, ScrubReport, SolveMode, exhaust_line, scrub_heuristic, skim_heuristic,
        skim_line,
    },
    puzzle::{
        BACKGROUND, Clue, Color, ColorInfo, DynSolution, PartialSolution, Puzzle, Solution,
        UNSOLVED,
    },
};

pub struct SolveOptions {
    pub trace_solve: bool,
    pub display_cli_progress: bool,
    pub only_solve_color: Option<Color>,
    pub max_effort: SolveMode,
}

impl Default for SolveOptions {
    fn default() -> Self {
        SolveOptions {
            trace_solve: false,
            display_cli_progress: false,
            only_solve_color: None,
            max_effort: SolveMode::Scrub,
        }
    }
}

pub type LineStatus = anyhow::Result<Option<SolveMode>>;

pub struct Report {
    pub solve_counts: ModeMap<usize>,
    pub cells_left: usize,
    pub solution: DynSolution,
    /// One entry per cell, in the dense order the geometry defines — the same indexing as
    /// `PartialSolution` and `Solution::cells`.
    pub solved_mask: Vec<bool>,
}

#[derive(Clone, Copy, Debug)]
struct PerModeLaneState {
    processed: bool,
    score: i32,
    processed_score: i32,
}

impl PerModeLaneState {
    fn new() -> PerModeLaneState {
        PerModeLaneState {
            processed: false,
            score: 0,
            processed_score: 0,
        }
    }
}

pub struct LaneState<'a, C: Clue> {
    clues: &'a [C], // just convenience, since `lane` suffices to find it again
    /// Index into `LaneMap::lanes()`.
    lane: usize,
    family: usize,
    /// Position within the family, for display only.
    index_in_family: usize,
    per_mode: ModeMap<PerModeLaneState>,
}

/// Family 0 is rows, 1 is columns; a triangular puzzle adds `/` and `\` lines.
fn family_letter(family: usize) -> char {
    ['R', 'C', 'D'][family]
}

impl<C: Clue> Debug for LaneState<'_, C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {:?}", self.text_coord(), self.clues)
    }
}

impl<'a, C: Clue> LaneState<'a, C> {
    pub fn text_coord(&self) -> String {
        format!("{}{}", family_letter(self.family), self.index_in_family + 1)
    }

    fn new(
        clues: &'a [C],
        lanes: &LaneMap,
        lane: usize,
        index_in_family: usize,
        grid: &PartialSolution,
        scratch: &mut Vec<Cell>,
    ) -> LaneState<'a, C> {
        let mut res = LaneState {
            clues,
            lane,
            family: lanes.lane(lane).family,
            index_in_family,
            per_mode: ModeMap::new_uniform(PerModeLaneState::new()),
        };
        res.rescore(lanes, grid, false, scratch);
        res
    }

    fn rescore(
        &mut self,
        lanes: &LaneMap,
        grid: &PartialSolution,
        was_processed: bool,
        scratch: &mut Vec<Cell>,
    ) {
        gather_into(lanes, self.lane, grid, scratch);
        let lane = ArrayView1::from(&scratch[..]);
        if lane.iter().all(|cell| cell.is_known()) {
            for mode in SolveMode::all() {
                self.per_mode[*mode].score = std::i32::MIN;
            }
            return;
        }

        for mode in SolveMode::all() {
            let s = &mut self.per_mode[*mode];
            if was_processed {
                s.processed_score = s.score;
            }
            s.score = match mode {
                SolveMode::Scrub => scrub_heuristic(self.clues, lane),
                SolveMode::Skim => skim_heuristic(self.clues, lane),
            };
        }
    }

    fn effective_score(&self, mode: SolveMode) -> i32 {
        let s = &self.per_mode[mode];
        s.score.saturating_sub(s.processed_score)
    }
}

/// Copy a lane's cells out of the grid into a contiguous buffer, so that the geometry-agnostic
/// line solvers in `line_solve` can work on it as a plain 1-D array.
///
/// `buf` is reused across calls on purpose. It is tempting to return a fresh `Array1` instead, but
/// the copy itself is cheap and the *allocation* is not: skim-only puzzles do no scrubbing and
/// keep no line cache, so this gather and the one in `rescore` are the only per-operation work of
/// their size, and allocating for each one costs ~9% on such puzzles.
fn gather_into(lanes: &LaneMap, lane: usize, grid: &PartialSolution, buf: &mut Vec<Cell>) {
    buf.clear();
    buf.extend(lanes.lane(lane).cells.iter().map(|c| grid[*c as usize]));
}

/// The inverse of `gather_into`.
fn scatter(lanes: &LaneMap, lane: usize, buf: &[Cell], grid: &mut PartialSolution) {
    for (position, cell) in lanes.lane(lane).cells.iter().enumerate() {
        grid[*cell as usize] = buf[position];
    }
}

fn find_best_lane<'a, 'b, C: Clue>(
    lanes: &'b mut [LaneState<'a, C>],
    mode: SolveMode,
) -> Option<&'b mut LaneState<'a, C>> {
    let mut best_score = std::i32::MIN;
    let mut res = None;

    for lane in lanes {
        if lane.per_mode[mode].processed {
            continue;
        }

        if lane.effective_score(mode) > best_score {
            best_score = lane.effective_score(mode);
            res = Some(lane);
        }
    }
    res
}

fn grid_to_solved_mask(grid: &PartialSolution) -> Vec<bool> {
    grid.iter().map(|cell| cell.is_known()).collect()
}

fn grid_to_solution<C: Clue, K: GridKind>(
    grid: &PartialSolution,
    puzzle: &Puzzle<C, K>,
) -> Solution<K> {
    let mut palette = puzzle.palette.clone();
    if grid.iter().any(|cell| !cell.is_known()) {
        palette.insert(
            UNSOLVED,
            ColorInfo {
                ch: '?',
                name: "unsolved".to_owned(),
                rgb: (128, 128, 128),
                color: UNSOLVED,
                corner: None,
            },
        );
    }
    let cells: Vec<Color> = grid
        .iter()
        .map(|cell| cell.known_or().unwrap_or(UNSOLVED))
        .collect();
    Solution::new(C::style(), palette, puzzle.geometry.clone(), cells)
}

/// `Report` is kind-erased because it crosses the dynamic boundary back to the GUI and the CLI.
fn dyn_solution<C: Clue, K: GridKind>(
    grid: &PartialSolution,
    puzzle: &Puzzle<C, K>,
) -> DynSolution {
    K::wrap_solution(grid_to_solution(grid, puzzle))
}

fn display_step<'a, C: Clue, K: GridKind>(
    clue_lane: &'a LaneState<'a, C>,
    orig_lane: Vec<Cell>,
    mode: SolveMode,
    grid: &'a PartialSolution,
    puzzle: &'a Puzzle<C, K>,
) {
    use std::fmt::Write;
    let mut clues = String::new();

    for clue in clue_lane.clues {
        write!(clues, "{} ", clue.to_string(&puzzle.palette)).unwrap();
    }

    print!(
        "{: <4} {: >16} {} ",
        clue_lane.text_coord(),
        clues,
        mode.ch()
    );

    let mut now_lane = vec![];
    gather_into(
        puzzle.geometry.lane_map(),
        clue_lane.lane,
        grid,
        &mut now_lane,
    );
    for (orig, now) in orig_lane.iter().zip(now_lane.iter()) {
        let new_ch = match now.known_or() {
            None => "?".to_string(),
            Some(known_color) => puzzle.palette[&known_color].ch.to_string(),
        };

        if *orig != *now {
            print!("{}", new_ch.underline());
        } else {
            print!("{}", new_ch);
        }
    }

    // Hackish way of getting the original score...
    let lane_arr: ndarray::Array1<Cell> = orig_lane.into();
    let (orig_score, new_score) = match mode {
        SolveMode::Scrub => (
            scrub_heuristic(clue_lane.clues, lane_arr.rows().into_iter().next().unwrap()),
            clue_lane.per_mode[mode].score,
        ),
        SolveMode::Skim => (
            skim_heuristic(clue_lane.clues, lane_arr.rows().into_iter().next().unwrap()),
            clue_lane.per_mode[mode].score,
        ),
    };
    println!("   {}->{}", orig_score, new_score);
}

pub type LineCache<C> = std::collections::HashMap<(Vec<C>, Vec<u32>), (ScrubReport, Vec<Cell>)>;

fn op_or_cache<'a, C: Clue, F>(
    f: F,
    solve_lane: &LaneState<'a, C>,
    lane: &mut ArrayViewMut1<Cell>,
    cache: &mut Option<LineCache<C>>,
) -> anyhow::Result<ScrubReport>
where
    F: Fn(&[C], &mut ArrayViewMut1<Cell>) -> anyhow::Result<ScrubReport>,
{
    if let Some(cache) = cache {
        let entry = cache.entry((
            solve_lane.clues.to_vec(),
            lane.iter().map(|cell| cell.raw()).collect::<Vec<_>>(),
        ));
        match entry {
            std::collections::hash_map::Entry::Occupied(o) => {
                let (report, new_cells) = o.get();

                for (idx, new_cell) in report.affected_cells.iter().zip(new_cells) {
                    lane[*idx] = *new_cell;
                }

                return Ok(report.clone());
            }
            std::collections::hash_map::Entry::Vacant(v) => {
                let report = f(solve_lane.clues, lane)?;
                let mut cells_to_cache = vec![];

                for idx in &report.affected_cells {
                    cells_to_cache.push(lane[*idx]);
                }

                v.insert((report.clone(), cells_to_cache));
                return Ok(report);
            }
        }
    } else {
        f(solve_lane.clues, lane)
    }
}

pub fn solve<C: Clue, K: GridKind>(
    puzzle: &Puzzle<C, K>,
    line_cache: &mut Option<LineCache<C>>,
    options: &SolveOptions,
) -> anyhow::Result<Report> {
    let mut grid = vec![Cell::new(&puzzle.palette); puzzle.geometry.cell_count()];
    solve_grid(puzzle, line_cache, options, &mut grid)
}

pub fn settle_solution<C: Clue, K: GridKind>(
    puzzle: &Puzzle<C, K>,
    grid: &mut PartialSolution,
) -> anyhow::Result<()> {
    let mut buf: Vec<Cell> = vec![];
    for (lane, clues) in puzzle.lines.iter().enumerate() {
        gather_into(puzzle.geometry.lane_map(), lane, grid, &mut buf);
        crate::line_solve::settle_line(clues, &mut ArrayViewMut1::from(&mut buf[..]))?;
        scatter(puzzle.geometry.lane_map(), lane, &buf, grid);
    }
    Ok(())
}

pub fn solve_grid<C: Clue, K: GridKind>(
    puzzle: &Puzzle<C, K>,
    line_cache: &mut Option<LineCache<C>>,
    options: &SolveOptions,
    grid: &mut PartialSolution,
) -> anyhow::Result<Report> {
    let lanes = puzzle.geometry.lane_map();
    let mut solve_lanes = vec![];
    // Reused across every gather, so the solve loop does no per-operation allocation.
    let mut scratch: Vec<Cell> = vec![];
    let mut buf: Vec<Cell> = vec![];
    let mut stale: Vec<bool> = vec![];

    // `solve_lanes` is parallel to `geometry.lanes()`, so a lane index indexes both.
    for family in 0..lanes.family_count() {
        for (index_in_family, lane) in lanes.family(family).enumerate() {
            solve_lanes.push(LaneState::new(
                &puzzle.lines[lane],
                lanes,
                lane,
                index_in_family,
                &grid,
                &mut scratch,
            ));
        }
    }

    let progress = indicatif::ProgressBar::new_spinner();
    if options.trace_solve || !options.display_cli_progress {
        progress.finish_and_clear();
    }

    let mut cells_left = grid.iter().filter(|c| !c.is_known()).count();
    let mut solve_counts = ModeMap::new_uniform(0);

    let initial_allowed_failures = ModeMap {
        skim: 10,
        scrub: 0, /*ignored */
    };

    let mut allowed_failures = initial_allowed_failures;

    loop {
        progress.tick();
        let mut current_mode = options.max_effort;
        for mode in SolveMode::all() {
            if allowed_failures[*mode] > 0 {
                current_mode = std::cmp::min(current_mode, *mode);
                break;
            }
        }

        let (report, solved_lane) = {
            let best_clue_lane = match find_best_lane(&mut solve_lanes, current_mode) {
                Some(lane) => lane,
                None => {
                    if current_mode >= options.max_effort {
                        // Nothing left to try; can't solve.
                        return Ok(Report {
                            solve_counts,
                            cells_left,
                            solution: dyn_solution(&grid, puzzle),
                            solved_mask: grid_to_solved_mask(&grid),
                        });
                    } else {
                        allowed_failures[current_mode] = 0; // try the next mode
                        continue;
                    }
                }
            };

            progress.set_message(format!(
                "{solve_counts} cells left: {cells_left: >6}  {}ing {}",
                current_mode.colorized_name(),
                best_clue_lane.text_coord(),
            ));

            // Pull the lane out of the grid so the line solvers see a plain 1-D array.
            gather_into(lanes, best_clue_lane.lane, grid, &mut buf);
            let orig_version_of_line: Vec<Cell> = buf.clone();
            let mut best_grid_lane: ArrayViewMut1<Cell> = ArrayViewMut1::from(&mut buf[..]);

            solve_counts[current_mode] += 1;
            let mut report = match current_mode {
                SolveMode::Scrub => op_or_cache(
                    exhaust_line,
                    best_clue_lane,
                    &mut best_grid_lane,
                    line_cache,
                )
                .context(format!(
                    "scrubbing {:?} with {:?}",
                    best_clue_lane, orig_version_of_line
                ))?,
                SolveMode::Skim => {
                    skim_line(best_clue_lane.clues, &mut best_grid_lane).context(format!(
                        "skimming {:?} with {:?}",
                        best_clue_lane, orig_version_of_line
                    ))?
                }
            };
            best_clue_lane.per_mode[current_mode].processed = true;

            if let Some(color) = options.only_solve_color {
                crate::line_solve::filter_report_by_color(
                    &mut report,
                    &orig_version_of_line,
                    &mut best_grid_lane,
                    color,
                );
            }

            let known_before = orig_version_of_line.iter().filter(|c| c.is_known()).count();
            let known_after = best_grid_lane.iter().filter(|c| c.is_known()).count();

            scatter(lanes, best_clue_lane.lane, &buf, grid);
            best_clue_lane.rescore(lanes, grid, /*was_processed=*/ true, &mut scratch);

            cells_left -= known_after - known_before;

            if options.trace_solve {
                display_step(
                    best_clue_lane,
                    orig_version_of_line,
                    current_mode,
                    grid,
                    puzzle,
                );
            }

            (report, best_clue_lane.lane)
        };

        if cells_left == 0 {
            progress.finish_and_clear();
            return Ok(Report {
                solve_counts,
                cells_left,
                solution: dyn_solution(&grid, puzzle),
                solved_mask: grid_to_solved_mask(&grid),
            });
        }

        if current_mode != SolveMode::first() && !report.affected_cells.is_empty() {
            // Made progress: reset and try easy stuff first again.
            allowed_failures = initial_allowed_failures;
        }

        if current_mode != options.max_effort {
            if report.affected_cells.is_empty() {
                allowed_failures[current_mode] -= 1;
            } else {
                allowed_failures[current_mode] =
                    std::cmp::min(10, allowed_failures[current_mode] + 1);
            }
        }

        // Affected intersecting lanes now may need to be re-examined.
        //
        // `report.affected_cells` holds positions *within the lane we just solved*, so translate
        // them into cell indices and ask the geometry which other lanes hold those cells. On a
        // square grid this is the same set as the old "column `i` meets row `j` at position `i`"
        // shortcut, but that shortcut doesn't survive a third axis: lanes can meet at a position
        // unrelated to their index, and two lanes can share more than one cell.
        stale.clear();
        stale.resize(solve_lanes.len(), false);
        for position in &report.affected_cells {
            let cell = lanes.lane(solved_lane).cells[*position];
            for membership in lanes.memberships(cell) {
                if membership.lane as usize != solved_lane {
                    stale[membership.lane as usize] = true;
                }
            }
        }

        for (other_lane, is_stale) in solve_lanes.iter_mut().zip(&stale) {
            if *is_stale {
                other_lane.rescore(lanes, &grid, /*was_processed=*/ false, &mut scratch);
                for mode in SolveMode::all() {
                    other_lane.per_mode[*mode].processed = false;
                }
            }
        }
    }
}

fn analyze_line<C: Clue>(clues: &[C], lane: ArrayView1<Cell>) -> LineStatus {
    let any_newly_known = |original_lane: ArrayView1<Cell>, new_lane: ArrayView1<Cell>| -> bool {
        original_lane
            .iter()
            .zip(new_lane.iter())
            .any(|(orig, new)| !orig.is_known() && new.is_known())
    };

    // Try skimming
    let mut skim_lane = lane.to_owned();
    skim_line(clues, &mut skim_lane.view_mut())?;
    if any_newly_known(lane, skim_lane.view()) {
        return Ok(Some(SolveMode::Skim));
    }

    // Try scrubbing
    let mut scrub_lane = lane.to_owned();
    exhaust_line(clues, &mut scrub_lane.view_mut())?;
    if any_newly_known(lane, scrub_lane.view()) {
        return Ok(Some(SolveMode::Scrub));
    }

    Ok(None)
}

pub fn analyze_lines<C: Clue, K: GridKind>(
    puzzle: &Puzzle<C, K>,
    grid: &PartialSolution,
) -> (Vec<LineStatus>, Vec<LineStatus>) {
    // TODO: return one `Vec` per family, rather than a pair, once the GUI can display a third.
    let mut per_family = vec![];
    let lanes = puzzle.geometry.lane_map();
    for family in 0..lanes.family_count() {
        per_family.push(
            lanes
                .family(family)
                .map(|lane| {
                    let mut gathered = vec![];
                    gather_into(lanes, lane, grid, &mut gathered);
                    analyze_line(&puzzle.lines[lane], ArrayView1::from(&gathered[..]))
                })
                .collect::<Vec<_>>(),
        );
    }

    let mut families = per_family.into_iter();
    let rows = families.next().unwrap_or_default();
    let cols = families.next().unwrap_or_default();
    (rows, cols)
}

pub enum DisambigResult {
    // The puzzle is already fully solvable without any extra cells: disambiguation is a no-op.
    Unnecessary,
    /// One entry per cell, in the dense order the geometry defines.
    Report(Vec<(Color, f32)>),
}

pub async fn disambig_candidates(
    s: &DynSolution,
    progress: mpsc::Sender<f32>,
    terminate: mpsc::Receiver<()>,
) -> DisambigResult {
    let mut solve_cache = crate::puzzle::DynSolveCache::new();

    let p = s.to_puzzle();
    // Probably redundant, but a small cost compared to the rest!
    let Report {
        cells_left: orig_cells_left,
        ..
    } = solve_cache
        .solve(&p)
        .expect("started from a solution; shouldn't be possible!");

    let cell_count = s.cells().len();
    let mut res = vec![(BACKGROUND, 0.0); cell_count];
    if orig_cells_left == 0 {
        progress.send(0.0).unwrap();
        return DisambigResult::Unnecessary;
    }

    for cell in 0..cell_count {
        let mut best_result = std::usize::MAX;
        let mut best_color = BACKGROUND;

        for new_col in s.palette().keys() {
            if *new_col == s.cells()[cell] {
                continue;
            }
            let mut new_solution = s.clone();
            new_solution.cells_mut()[cell] = *new_col;

            let Report {
                cells_left: new_cells_left,
                ..
            } = solve_cache.solve(&new_solution.to_puzzle()).expect("");

            if new_cells_left < best_result {
                best_result = new_cells_left;
                best_color = *new_col;
            }
        }

        if cell % 5 == 0 {
            progress.send(cell as f32 / cell_count as f32).unwrap();
        }

        gui::yield_now().await;

        res[cell] = (best_color, (best_result as f32) / (orig_cells_left as f32));

        if terminate.try_recv().is_ok() {
            return DisambigResult::Report(res);
        }
    }
    progress.send(1.0).unwrap();

    DisambigResult::Report(res)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::puzzle::{ColorInfo, Nono};

    use super::*;

    #[test]
    fn test_analyze_lines() {
        let mut palette = HashMap::new();
        palette.insert(BACKGROUND, ColorInfo::default_bg());
        palette.insert(Color(1), ColorInfo::default_fg(Color(1)));

        let clue = |n| {
            vec![Nono {
                color: Color(1),
                count: n,
            }]
        };
        let puzzle = Puzzle::square(
            palette,
            vec![clue(1), clue(1)],
            vec![clue(1), clue(2)], // impossible
        );

        let mut grid = vec![Cell::new(&puzzle.palette); 4];
        grid[0] = Cell::from_color(BACKGROUND); // (x=0, y=0)
        grid[3] = Cell::from_color(BACKGROUND); // (x=1, y=1)

        let (row_tech, col_tech) = analyze_lines(&puzzle, &grid);

        assert_eq!(
            row_tech.into_iter().map(|r| r.ok()).collect::<Vec<_>>(),
            vec![Some(Some(SolveMode::Skim)), Some(Some(SolveMode::Skim))]
        );
        assert!(col_tech[0].as_ref().is_ok());
        assert!(col_tech[1].is_err());
    }

    #[test]
    fn test_solution_to_grid() {
        let mut palette = HashMap::new();
        palette.insert(BACKGROUND, ColorInfo::default_bg());
        palette.insert(Color(1), ColorInfo::default_fg(Color(1)));

        let puzzle: Puzzle<Nono, crate::geometry::Square> =
            Puzzle::square(palette, vec![vec![]], vec![vec![]]);

        let solution = Solution::from_columns(
            crate::puzzle::ClueStyle::Nono,
            puzzle.palette.clone(),
            vec![vec![BACKGROUND, UNSOLVED]],
        );

        // One column of two cells, so the flat indices are (x=0, y=0) and (x=0, y=1).
        let grid = solution.to_partial();
        assert!(grid[0].is_known_to_be(BACKGROUND));
        assert!(!grid[1].is_known());
        assert!(grid[1].can_be(BACKGROUND));
        assert!(grid[1].can_be(Color(1)));
    }

    #[test]
    fn test_color_filtered_solve() {
        // A bare lane with nothing crossing it, so the row clue is the only constraint.
        let puz = Puzzle::single_lane(
            HashMap::new(), // ignored!
            7,
            vec![Nono {
                color: Color(1),
                count: 3,
            }],
        );
        let mut grid = vec![Cell::new_anything(); 7];
        grid[5] = Cell::from_color(Color(1));

        let bkg_solved = solve_grid(
            &puz,
            &mut None,
            &SolveOptions {
                only_solve_color: Some(BACKGROUND),
                max_effort: SolveMode::Skim,
                ..SolveOptions::default()
            },
            &mut grid,
        )
        .unwrap();

        assert_eq!(bkg_solved.cells_left, 3);

        assert_eq!(
            grid,
            vec![
                Cell::from_color(BACKGROUND),
                Cell::from_color(BACKGROUND),
                Cell::from_color(BACKGROUND),
                Cell::new_anything(),
                Cell::new_anything(), // Known to be 1, but not allowed to say it
                Cell::from_color(Color(1)),
                Cell::new_anything()
            ]
        )
    }

    /// Derive clues for every lane of a triangular puzzle from a filled/empty pattern, the way
    /// `solution_to_puzzle` does for square ones.
    fn triangular_puzzle(
        outline: crate::geometry::Outline,
        filled: &[bool],
    ) -> Puzzle<Nono, crate::geometry::Tri> {
        let geometry = crate::geometry::Geometry::<crate::geometry::Tri>::new(outline);
        assert_eq!(filled.len(), geometry.cell_count());

        let mut palette = HashMap::new();
        palette.insert(BACKGROUND, ColorInfo::default_bg());
        palette.insert(Color(1), ColorInfo::default_fg(Color(1)));

        let mut lines = vec![];
        for lane in 0..geometry.lane_count() {
            let mut clues: Vec<Nono> = vec![];
            let mut run = 0u16;
            for cell in &geometry.lane(lane).cells {
                if filled[*cell as usize] {
                    run += 1;
                } else if run > 0 {
                    clues.push(Nono {
                        color: Color(1),
                        count: run,
                    });
                    run = 0;
                }
            }
            if run > 0 {
                clues.push(Nono {
                    color: Color(1),
                    count: run,
                });
            }
            lines.push(clues);
        }

        Puzzle {
            palette,
            geometry,
            lines,
        }
    }

    /// Solve a triangular puzzle and confirm it never contradicts the pattern it came from.
    /// Returns how many cells it couldn't pin down.
    fn solve_triangular(outline: crate::geometry::Outline, filled: &[bool]) -> usize {
        let puzzle = triangular_puzzle(outline, filled);
        let report = solve(&puzzle, &mut None, &SolveOptions::default()).unwrap();

        let mut grid = vec![Cell::new(&puzzle.palette); puzzle.geometry.cell_count()];
        solve_grid(&puzzle, &mut None, &SolveOptions::default(), &mut grid).unwrap();

        for (cell, cell_filled) in grid.iter().zip(filled) {
            let truth = if *cell_filled { Color(1) } else { BACKGROUND };
            assert!(
                cell.can_be(truth),
                "solver ruled out the real colour of a cell"
            );
        }
        report.cells_left
    }

    #[test]
    fn solves_a_uniform_triangular_puzzle() {
        for side in 1..=3 {
            let outline = crate::geometry::Outline::hexagon(side);
            let count =
                crate::geometry::Geometry::<crate::geometry::Tri>::new(outline).cell_count();
            assert_eq!(solve_triangular(outline, &vec![true; count]), 0);
            assert_eq!(solve_triangular(outline, &vec![false; count]), 0);
        }
    }

    #[test]
    fn solves_triangular_puzzles_soundly() {
        // A handful of deterministic pseudo-random patterns. Not every one is uniquely
        // determined by its clues, so this checks soundness rather than completeness.
        for outline in [
            crate::geometry::Outline::hexagon(2),
            crate::geometry::Outline::hexagon(3),
            // The webpbn worked example: a shape with an off-centre bend.
            crate::geometry::Outline {
                a: (0, 2),
                b: (1, 3),
                c: (-1, 2),
            },
        ] {
            let count =
                crate::geometry::Geometry::<crate::geometry::Tri>::new(outline).cell_count();
            let mut seed = 0x2545F491u32;
            for _ in 0..20 {
                let filled: Vec<bool> = (0..count)
                    .map(|_| {
                        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                        seed >> 30 != 0
                    })
                    .collect();
                solve_triangular(outline, &filled);
            }
        }
    }

    /// The pattern that breaks the square-grid assumption: a ▲ and the ▼ to its right share both
    /// a row and a `/` line, so invalidation driven by lane *index* would miss updates.
    #[test]
    fn solves_the_doc_example_outline_completely() {
        let outline = crate::geometry::Outline {
            a: (0, 2),
            b: (1, 3),
            c: (-1, 2),
        };
        // Fill the middle row only.
        let geometry = crate::geometry::Geometry::<crate::geometry::Tri>::new(outline);
        let middle: std::collections::HashSet<u32> =
            geometry.lane(1).cells.iter().copied().collect();
        let filled: Vec<bool> = (0..geometry.cell_count() as u32)
            .map(|c| middle.contains(&c))
            .collect();

        assert_eq!(solve_triangular(outline, &filled), 0);
    }

    #[test]
    fn test_settle_solution() {
        let mut palette = HashMap::new();
        palette.insert(BACKGROUND, ColorInfo::default_bg());
        palette.insert(Color(1), ColorInfo::default_fg(Color(1)));

        let clue = |n| {
            vec![Nono {
                color: Color(1),
                count: n,
            }]
        };
        let puzzle = Puzzle::square(palette, vec![clue(1), clue(1)], vec![clue(1), clue(1)]);

        let mut grid = vec![Cell::new(&puzzle.palette); 4];
        grid[0] = Cell::from_color(Color(1)); // (x=0, y=0)
        grid[3] = Cell::from_color(Color(1)); // (x=1, y=1)

        settle_solution(&puzzle, &mut grid).unwrap();

        assert!(grid[1].is_known_to_be(BACKGROUND)); // (x=1, y=0)
        assert!(grid[2].is_known_to_be(BACKGROUND)); // (x=0, y=1)
    }
}
