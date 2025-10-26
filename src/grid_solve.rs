use std::{fmt::Debug, sync::mpsc, vec};

use anyhow::Context;
use colored::Colorize;
use ndarray::{ArrayView1, ArrayViewMut1};

use crate::{
    gui,
    line_solve::{
        Cell, ModeMap, ScrubReport, SolveMode, exhaust_line, scrub_heuristic, skim_heuristic,
        skim_line,
    },
    puzzle::{BACKGROUND, Clue, Color, Puzzle, Solution, UNSOLVED},
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

type Grid = ndarray::Array2<Cell>;
pub type LineStatus = anyhow::Result<Option<SolveMode>>;

pub struct Report {
    pub solve_counts: ModeMap<usize>,
    pub cells_left: usize,
    pub solution: Solution,
    pub solved_mask: Vec<Vec<bool>>,
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
    clues: &'a [C], // just convenience, since `row` and `index` suffice to find it again
    row: bool,
    index: ndarray::Ix,
    per_mode: ModeMap<PerModeLaneState>,
}

impl<C: Clue> Debug for LaneState<'_, C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}{}: {:?}",
            if self.row { "R" } else { "C" },
            self.index + 1,
            self.clues
        )
    }
}

impl<'a, C: Clue> LaneState<'a, C> {
    pub fn text_coord(&self) -> String {
        format!("{}{}", if self.row { "R" } else { "C" }, self.index + 1)
    }

    fn new(clues: &'a [C], row: bool, idx: usize, grid: &Grid) -> LaneState<'a, C> {
        let mut res = LaneState {
            clues,
            row,
            index: idx,
            per_mode: ModeMap::new_uniform(PerModeLaneState::new()),
        };
        res.rescore(grid, false);
        res
    }
    fn rescore(&mut self, grid: &Grid, was_processed: bool) {
        let lane = get_grid_lane(self, grid);
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

fn get_mut_grid_lane<'a, C: Clue>(
    ls: &LaneState<'a, C>,
    grid: &'a mut Grid,
) -> ArrayViewMut1<'a, Cell> {
    if ls.row {
        grid.row_mut(ls.index)
    } else {
        grid.column_mut(ls.index)
    }
}

fn get_grid_lane<'a, C: Clue>(ls: &LaneState<'a, C>, grid: &'a Grid) -> ArrayView1<'a, Cell> {
    if ls.row {
        grid.row(ls.index)
    } else {
        grid.column(ls.index)
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

fn grid_to_solved_mask<C: Clue>(grid: &Grid) -> Vec<Vec<bool>> {
    grid.columns()
        .into_iter()
        .map(|col| {
            col.iter()
                .map(|cell| cell.is_known())
                .collect::<Vec<bool>>()
        })
        .collect()
}

fn grid_to_solution<C: Clue>(grid: &Grid, puzzle: &Puzzle<C>) -> Solution {
    let grid = grid
        .columns()
        .into_iter()
        .map(|col| {
            col.iter()
                .map(|cell| cell.known_or().unwrap_or(BACKGROUND))
                .collect::<Vec<Color>>()
        })
        .collect();
    Solution {
        clue_style: C::style(),
        grid,
        palette: puzzle.palette.clone(),
    }
}

fn display_step<'a, C: Clue>(
    clue_lane: &'a LaneState<'a, C>,
    orig_lane: Vec<Cell>,
    mode: SolveMode,
    grid: &'a Grid,
    puzzle: &'a Puzzle<C>,
) {
    use std::fmt::Write;
    let mut clues = String::new();

    for clue in clue_lane.clues {
        write!(clues, "{} ", clue.to_string(puzzle)).unwrap();
    }

    let r_or_c = if clue_lane.row { "R" } else { "C" };

    print!(
        "{}{: <3} {: >16} {} ",
        r_or_c,
        clue_lane.index,
        clues,
        mode.ch()
    );

    for (orig, now) in orig_lane.iter().zip(get_grid_lane(clue_lane, grid)) {
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

pub fn grid_from_solution<C: Clue>(solution: &Solution, puzzle: &Puzzle<C>) -> Grid {
    let mut grid = Grid::from_elem(
        (solution.x_size(), solution.y_size()),
        Cell::new_impossible(),
    );
    for (x, row) in solution.grid.iter().enumerate() {
        for (y, color) in row.iter().enumerate() {
            if *color == UNSOLVED {
                grid[[x, y]] = Cell::new(puzzle);
            } else {
                grid[[x, y]] = Cell::from_color(*color);
            }
        }
    }
    grid
}

pub fn solve<C: Clue>(
    puzzle: &Puzzle<C>,
    line_cache: &mut Option<LineCache<C>>,
    options: &SolveOptions,
) -> anyhow::Result<Report> {
    let mut grid = Grid::from_elem((puzzle.rows.len(), puzzle.cols.len()), Cell::new(puzzle));
    solve_grid(puzzle, line_cache, options, &mut grid)
}

pub fn solve_grid<C: Clue>(
    puzzle: &Puzzle<C>,
    line_cache: &mut Option<LineCache<C>>,
    options: &SolveOptions,
    grid: &mut Grid,
) -> anyhow::Result<Report> {
    let mut solve_lanes = vec![];

    for (idx, clue_row) in puzzle.rows.iter().enumerate() {
        solve_lanes.push(LaneState::new(clue_row, true, idx, &grid));
    }

    for (idx, clue_col) in puzzle.cols.iter().enumerate() {
        solve_lanes.push(LaneState::new(clue_col, false, idx, &grid));
    }

    let progress = indicatif::ProgressBar::new_spinner();
    if options.trace_solve || !options.display_cli_progress {
        progress.finish_and_clear();
    }

    let mut cells_left = puzzle.rows.len() * puzzle.cols.len();
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

        let (report, was_row) = {
            let best_clue_lane = match find_best_lane(&mut solve_lanes, current_mode) {
                Some(lane) => lane,
                None => {
                    if current_mode >= options.max_effort {
                        // Nothing left to try; can't solve.
                        return Ok(Report {
                            solve_counts,
                            cells_left,
                            solution: grid_to_solution::<C>(&grid, puzzle),
                            solved_mask: grid_to_solved_mask::<C>(&grid),
                        });
                    } else {
                        allowed_failures[current_mode] = 0; // try the next mode
                        continue;
                    }
                }
            };

            let mut best_grid_lane: ArrayViewMut1<Cell> =
                get_mut_grid_lane(best_clue_lane, grid);

            progress.set_message(format!(
                "{solve_counts} cells left: {cells_left: >6}  {}ing {}",
                current_mode.colorized_name(),
                best_clue_lane.text_coord(),
            ));

            let orig_version_of_line: Vec<Cell> = best_grid_lane.iter().cloned().collect();

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
                filter_report_by_color(
                    &mut report,
                    &orig_version_of_line,
                    &mut best_grid_lane,
                    color,
                );
            }

            let known_before = orig_version_of_line.iter().filter(|c| c.is_known()).count();
            let known_after = best_grid_lane.iter().filter(|c| c.is_known()).count();

            best_clue_lane.rescore(grid, /*was_processed=*/ true);

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

            (report, best_clue_lane.row)
        };

        if cells_left == 0 {
            progress.finish_and_clear();
            return Ok(Report {
                solve_counts,
                cells_left,
                solution: grid_to_solution::<C>(&grid, puzzle),
                solved_mask: grid_to_solved_mask::<C>(&grid),
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

        // Affected intersecting lanes now may need to be re-examined:
        for other_lane in solve_lanes.iter_mut() {
            if other_lane.row != was_row && report.affected_cells.contains(&other_lane.index) {
                other_lane.rescore(&grid, /*was_processed=*/ false);
                for mode in SolveMode::all() {
                    other_lane.per_mode[*mode].processed = false;
                }
            }
        }
    }
}

fn filter_report_by_color(
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

pub fn analyze_lines<C: Clue>(
    puzzle: &Puzzle<C>,
    grid: &Grid,
) -> (Vec<LineStatus>, Vec<LineStatus>) {
    let mut row_techniques = vec![];
    for (idx, clues) in puzzle.rows.iter().enumerate() {
        row_techniques.push(analyze_line(clues, grid.row(idx)));
    }

    let mut col_techniques = vec![];
    for (idx, clues) in puzzle.cols.iter().enumerate() {
        col_techniques.push(analyze_line(clues, grid.column(idx)));
    }

    (row_techniques, col_techniques)
}

pub async fn disambig_candidates(
    s: &Solution,
    progress: mpsc::Sender<f32>,
    terminate: mpsc::Receiver<()>,
) -> Vec<Vec<(Color, f32)>> {
    let mut solve_cache = crate::puzzle::DynSolveCache::new();

    let p = s.to_puzzle();
    // Probably redundant, but a small cost compared to the rest!
    let Report {
        cells_left: orig_cells_left,
        ..
    } = solve_cache
        .solve(&p)
        .expect("started from a solution; shouldn't be possible!");

    let mut res = vec![vec![(BACKGROUND, 0.0); s.grid.first().unwrap().len()]; s.grid.len()];
    if orig_cells_left == 0 {
        // TODO: probably send a result
        progress.send(0.0).unwrap();
        return res;
    }

    for x in 0..s.x_size() {
        for y in 0..s.y_size() {
            let mut best_result = std::usize::MAX;
            let mut best_color = BACKGROUND;

            for new_col in s.palette.keys() {
                if *new_col == s.grid[x][y] {
                    continue;
                }
                let mut new_grid = s.grid.clone();
                new_grid[x][y] = *new_col;
                let new_solution = Solution {
                    grid: new_grid,
                    ..s.clone()
                };

                let Report {
                    cells_left: new_cells_left,
                    ..
                } = solve_cache.solve(&new_solution.to_puzzle()).expect("");

                if new_cells_left < best_result {
                    best_result = new_cells_left;
                    best_color = *new_col;
                }
            }

            if y % 5 == 0 {
                progress
                    .send((x * s.y_size() + y) as f32 / (s.x_size() * s.y_size()) as f32)
                    .unwrap();
            }

            gui::yield_now().await;

            res[x][y] = (best_color, (best_result as f32) / (orig_cells_left as f32));

            if terminate.try_recv().is_ok() {
                return res;
            }
        }
    }
    progress.send(1.0).unwrap();

    return res;
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
        let puzzle = Puzzle {
            palette,
            rows: vec![clue(1), clue(1)],
            cols: vec![clue(1), clue(2)], // impossible
        };

        let mut grid = Grid::from_elem((2, 2), Cell::new(&puzzle));
        grid[[0, 0]] = Cell::from_color(BACKGROUND);
        grid[[1, 1]] = Cell::from_color(BACKGROUND);

        let (row_tech, col_tech) = analyze_lines(&puzzle, &grid);

        assert_eq!(
            row_tech.into_iter().map(|r| r.ok()).collect::<Vec<_>>(),
            vec![Some(Some(SolveMode::Skim)), Some(Some(SolveMode::Skim))]
        );
        assert!(col_tech[0].as_ref().is_ok());
        assert!(col_tech[1].is_err());
    }

    #[test]
    fn test_grid_from_solution() {
        let mut palette = HashMap::new();
        palette.insert(BACKGROUND, ColorInfo::default_bg());
        palette.insert(Color(1), ColorInfo::default_fg(Color(1)));

        let puzzle: Puzzle<Nono> = Puzzle {
            palette,
            rows: vec![vec![]],
            cols: vec![vec![]],
        };

        let solution = Solution {
            clue_style: crate::puzzle::ClueStyle::Nono,
            palette: puzzle.palette.clone(),
            grid: vec![vec![BACKGROUND, UNSOLVED]],
        };

        let grid = grid_from_solution(&solution, &puzzle);
        assert!(grid[[0, 0]].is_known_to_be(BACKGROUND));
        assert!(!grid[[0, 1]].is_known());
        assert!(grid[[0, 1]].can_be(BACKGROUND));
        assert!(grid[[0, 1]].can_be(Color(1)));
    }
}
