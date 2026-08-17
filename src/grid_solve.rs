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

#[derive(Clone)]
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

/// Returns an index into `lanes`, which is parallel to `LaneMap::lanes()`.
fn find_best_lane<C: Clue>(lanes: &[LaneState<'_, C>], mode: SolveMode) -> Option<usize> {
    let mut best_score = std::i32::MIN;
    let mut res = None;

    for (idx, lane) in lanes.iter().enumerate() {
        if lane.per_mode[mode].processed {
            continue;
        }

        if lane.effective_score(mode) > best_score {
            best_score = lane.effective_score(mode);
            res = Some(idx);
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

fn display_step<C: Clue, K: GridKind>(
    clue_lane: &LaneState<'_, C>,
    orig_lane: Vec<Cell>,
    mode: SolveMode,
    grid: &PartialSolution,
    puzzle: &Puzzle<C, K>,
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

fn op_or_cache<C: Clue, F>(
    f: F,
    clues: &[C],
    lane: &mut ArrayViewMut1<Cell>,
    cache: &mut Option<LineCache<C>>,
) -> anyhow::Result<ScrubReport>
where
    F: Fn(&[C], &mut ArrayViewMut1<Cell>) -> anyhow::Result<ScrubReport>,
{
    if let Some(cache) = cache {
        let entry = cache.entry((
            clues.to_vec(),
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
                let report = f(clues, lane)?;
                let mut cells_to_cache = vec![];

                for idx in &report.affected_cells {
                    cells_to_cache.push(lane[*idx]);
                }

                v.insert((report.clone(), cells_to_cache));
                return Ok(report);
            }
        }
    } else {
        f(clues, lane)
    }
}


pub fn solve<C: Clue, K: GridKind>(
    puzzle: &Puzzle<C, K>,
    line_cache: &mut Option<LineCache<C>>,
    options: &SolveOptions,
) -> anyhow::Result<Report> {
    // TODO: merge this and `line_logic_solve`
    let mut grid = vec![Cell::new(&puzzle.palette); puzzle.geometry.cell_count()];
    line_logic_solve(puzzle, line_cache, options, &mut grid)
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

/// Buffers reused across a whole solve, so that no per-lane operation allocates (see the note on
/// `gather_into`). Nothing in here carries meaning from one operation to the next, which is why
/// it lives in the context rather than in `SolveState`: a branching search can share one of these
/// across every branch it explores.
#[derive(Default)]
pub struct Scratch {
    /// Gather buffer for scoring; live only within a single `rescore` call.
    score: Vec<Cell>,
    /// Working copy of the lane being solved: gathered out of the grid, mutated in place by the
    /// line solver, then scattered back.
    lane: Vec<Cell>,
    /// Per-lane "needs another look" marks. Two lanes can share more than one cell, so this
    /// dedupes: a lane gets rescored once per step however many affected cells it holds.
    stale: Vec<bool>,
}

/// Everything a solve needs that doesn't change as it progresses, plus the caches and buffers it
/// reuses. A branching search builds one of these and shares it across every branch.
pub struct SolveContext<'p, 'x, C: Clue, K: GridKind> {
    pub puzzle: &'p Puzzle<C, K>,
    pub options: &'x SolveOptions,
    pub line_cache: &'x mut Option<LineCache<C>>,
    pub scratch: Scratch,
    progress: indicatif::ProgressBar,
}

impl<'p, C: Clue, K: GridKind> SolveContext<'p, '_, C, K> {
    /// Borrowing the puzzle separately from everything else is what lets a `SolveState` hold
    /// clue references (`'p`) that outlive any particular borrow of the line cache.
    fn lane_map(&self) -> &'p LaneMap {
        self.puzzle.geometry.lane_map()
    }
}

impl<'p, 'x, C: Clue, K: GridKind> SolveContext<'p, 'x, C, K> {
    pub fn new(
        puzzle: &'p Puzzle<C, K>,
        line_cache: &'x mut Option<LineCache<C>>,
        options: &'x SolveOptions,
    ) -> SolveContext<'p, 'x, C, K> {
        let progress = indicatif::ProgressBar::new_spinner();
        if options.trace_solve || !options.display_cli_progress {
            progress.finish_and_clear();
        }

        SolveContext {
            puzzle,
            options,
            line_cache,
            scratch: Scratch::default(),
            progress,
        }
    }
}

/// What one call to `SolveState::step` accomplished.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Step {
    /// A lane was attempted; there may be more to do, so call `step` again.
    Attempted,
    /// Every cell is known.
    Solved,
    /// No mode has a lane left worth trying: line logic alone can't get any further. This is
    /// where a backtracking search forks the state and `guess`es.
    Stalled,
}

const INITIAL_ALLOWED_FAILURES: ModeMap<i32> = ModeMap {
    skim: 10,
    scrub: 0, /*ignored */
};

/// A solve in progress. Cloning one forks the search: hand the copy a `guess` and drive it with
/// `step` without disturbing the original.
#[derive(Clone)]
pub struct SolveState<'p, C: Clue> {
    pub grid: PartialSolution,
    /// Parallel to `LaneMap::lanes()`, so a lane index indexes both this and the geometry.
    lanes: Vec<LaneState<'p, C>>,
    pub cells_left: usize,
    pub solve_counts: ModeMap<usize>,
    /// How many more fruitless attempts each below-`max_effort` mode gets before we stop trying
    /// it and escalate. Reset whenever a harder mode does turn something up.
    allowed_failures: ModeMap<i32>,
}

impl<'p, C: Clue> SolveState<'p, C> {
    pub fn new<K: GridKind>(
        ctx: &mut SolveContext<'p, '_, C, K>,
        grid: PartialSolution,
    ) -> SolveState<'p, C> {
        let puzzle = ctx.puzzle;
        let lane_map = ctx.lane_map();
        let scratch = &mut ctx.scratch;

        let mut lanes = vec![];
        // `lanes` is parallel to `geometry.lanes()`, so a lane index indexes both.
        for family in 0..lane_map.family_count() {
            for (index_in_family, lane) in lane_map.family(family).enumerate() {
                lanes.push(LaneState::new(
                    &puzzle.lines[lane],
                    lane_map,
                    lane,
                    index_in_family,
                    &grid,
                    &mut scratch.score,
                ));
            }
        }

        SolveState {
            cells_left: grid.iter().filter(|c| !c.is_known()).count(),
            grid,
            lanes,
            solve_counts: ModeMap::new_uniform(0),
            allowed_failures: INITIAL_ALLOWED_FAILURES,
        }
    }

    /// Step until the puzzle is solved or line logic stalls.
    pub fn run<K: GridKind>(
        &mut self,
        ctx: &mut SolveContext<'p, '_, C, K>,
    ) -> anyhow::Result<Step> {
        loop {
            match self.step(ctx)? {
                Step::Attempted => (),
                done => return Ok(done),
            }
        }
    }

    /// Pick the next lane to attempt, escalating to a more thorough mode once the cheap ones stop
    /// paying off. `None` means every mode up to `max_effort` is exhausted.
    fn choose_lane(&mut self, max_effort: SolveMode) -> Option<(usize, SolveMode)> {
        loop {
            let mut mode = max_effort;
            for m in SolveMode::all() {
                if self.allowed_failures[*m] > 0 {
                    mode = std::cmp::min(mode, *m);
                    break;
                }
            }

            match find_best_lane(&self.lanes, mode) {
                Some(lane) => return Some((lane, mode)),
                // Nothing left to try; can't solve.
                None if mode >= max_effort => return None,
                // Nothing left for *this* mode; go around again and pick the next one up.
                None => self.allowed_failures[mode] = 0,
            }
        }
    }

    /// Do one unit of work: pick the most promising lane, run a line solver over it, write what
    /// it learned back into the grid, and mark every lane the changed cells cross.
    pub fn step<K: GridKind>(
        &mut self,
        ctx: &mut SolveContext<'p, '_, C, K>,
    ) -> anyhow::Result<Step> {
        let puzzle = ctx.puzzle;
        let options = ctx.options;
        let lane_map = ctx.lane_map();

        ctx.progress.tick();

        let Some((idx, mode)) = self.choose_lane(options.max_effort) else {
            return Ok(Step::Stalled);
        };
        let solved_lane = self.lanes[idx].lane;

        ctx.progress.set_message(format!(
            "{} cells left: {: >6}  {}ing {}",
            self.solve_counts,
            self.cells_left,
            mode.colorized_name(),
            self.lanes[idx].text_coord(),
        ));

        // Pull the lane out of the grid so the line solvers see a plain 1-D array.
        gather_into(lane_map, solved_lane, &self.grid, &mut ctx.scratch.lane);
        let orig_version_of_line: Vec<Cell> = ctx.scratch.lane.clone();
        let mut grid_lane: ArrayViewMut1<Cell> = ArrayViewMut1::from(&mut ctx.scratch.lane[..]);

        self.solve_counts[mode] += 1;
        let clues = self.lanes[idx].clues;
        let mut report = match mode {
            SolveMode::Scrub => op_or_cache(exhaust_line, clues, &mut grid_lane, ctx.line_cache)
                .with_context(|| {
                    format!(
                        "scrubbing {:?} with {:?}",
                        &self.lanes[idx], orig_version_of_line
                    )
                })?,
            SolveMode::Skim => skim_line(clues, &mut grid_lane).with_context(|| {
                format!(
                    "skimming {:?} with {:?}",
                    &self.lanes[idx], orig_version_of_line
                )
            })?,
        };
        self.lanes[idx].per_mode[mode].processed = true;

        if let Some(color) = options.only_solve_color {
            crate::line_solve::filter_report_by_color(
                &mut report,
                &orig_version_of_line,
                &mut grid_lane,
                color,
            );
        }

        let known_before = orig_version_of_line.iter().filter(|c| c.is_known()).count();
        let known_after = grid_lane.iter().filter(|c| c.is_known()).count();

        scatter(lane_map, solved_lane, &ctx.scratch.lane, &mut self.grid);
        self.lanes[idx].rescore(
            lane_map,
            &self.grid,
            /*was_processed=*/ true,
            &mut ctx.scratch.score,
        );

        self.cells_left -= known_after - known_before;

        if options.trace_solve {
            display_step(
                &self.lanes[idx],
                orig_version_of_line,
                mode,
                &self.grid,
                puzzle,
            );
        }

        if self.cells_left == 0 {
            return Ok(Step::Solved);
        }

        if mode != SolveMode::first() && !report.affected_cells.is_empty() {
            // Made progress: reset and try easy stuff first again.
            self.allowed_failures = INITIAL_ALLOWED_FAILURES;
        }

        if mode != options.max_effort {
            if report.affected_cells.is_empty() {
                self.allowed_failures[mode] -= 1;
            } else {
                self.allowed_failures[mode] = std::cmp::min(10, self.allowed_failures[mode] + 1);
            }
        }

        // `report.affected_cells` holds positions *within the lane we just solved*, so translate
        // them into cell indices before handing them to `invalidate`.
        let affected = report
            .affected_cells
            .iter()
            .map(|position| lane_map.lane(solved_lane).cells[*position]);
        // The lane we just solved was rescored above and must stay `processed`, or we'd pick it
        // straight back up.
        self.invalidate(affected, Some(solved_lane), lane_map, &mut ctx.scratch);

        Ok(Step::Attempted)
    }

    /// Assume `cell` is `color` and mark everything that assumption bears on, so that a `step`
    /// after a `Stalled` picks up where the stall left off.
    ///
    /// Errors if the assumption contradicts what's already known, leaving the state partly
    /// updated — guess on a clone, and throw the clone away if this fails.
    pub fn guess<K: GridKind>(
        &mut self,
        ctx: &mut SolveContext<'p, '_, C, K>,
        cell: u32,
        color: Color,
    ) -> anyhow::Result<()> {
        let lane_map = ctx.lane_map();

        if self.grid[cell as usize].learn(color)? {
            self.cells_left -= 1;
        }
        self.invalidate(std::iter::once(cell), None, lane_map, &mut ctx.scratch);
        // A guess is new information, so it's worth another cheap pass before escalating.
        self.allowed_failures = INITIAL_ALLOWED_FAILURES;

        Ok(())
    }

    /// Mark every lane through `cells` as worth re-examining: bring its scores up to date and
    /// clear its `processed` flags so `choose_lane` will consider it again. `already_current`, if
    /// given, is a lane that has just been rescored and should be left alone.
    ///
    /// We ask the geometry which lanes hold each cell rather than using the old "column `i` meets
    /// row `j` at position `i`" shortcut: on a square grid the two agree, but that shortcut
    /// doesn't survive a third axis, where lanes can meet at a position unrelated to their index
    /// and two lanes can share more than one cell.
    fn invalidate(
        &mut self,
        cells: impl Iterator<Item = u32>,
        already_current: Option<usize>,
        lane_map: &LaneMap,
        scratch: &mut Scratch,
    ) {
        scratch.stale.clear();
        scratch.stale.resize(self.lanes.len(), false);
        for cell in cells {
            for membership in lane_map.memberships(cell) {
                if Some(membership.lane as usize) != already_current {
                    scratch.stale[membership.lane as usize] = true;
                }
            }
        }

        for (other_lane, is_stale) in self.lanes.iter_mut().zip(&scratch.stale) {
            if *is_stale {
                other_lane.rescore(
                    lane_map,
                    &self.grid,
                    /*was_processed=*/ false,
                    &mut scratch.score,
                );
                for mode in SolveMode::all() {
                    other_lane.per_mode[*mode].processed = false;
                }
            }
        }
    }

    /// Package the state up for the GUI and the CLI, which see a kind-erased solution.
    pub fn report<K: GridKind>(&self, puzzle: &Puzzle<C, K>) -> Report {
        Report {
            solve_counts: self.solve_counts,
            cells_left: self.cells_left,
            solution: dyn_solution(&self.grid, puzzle),
            solved_mask: grid_to_solved_mask(&self.grid),
        }
    }
}

/// Perform a complete line-logic solve
pub fn line_logic_solve<C: Clue, K: GridKind>(
    puzzle: &Puzzle<C, K>,
    line_cache: &mut Option<LineCache<C>>,
    options: &SolveOptions,
    grid: &mut PartialSolution,
) -> anyhow::Result<Report> {
    let mut ctx = SolveContext::new(puzzle, line_cache, options);
    let mut state = SolveState::new(&mut ctx, std::mem::take(grid));

    let outcome = state.run(&mut ctx);
    ctx.progress.finish_and_clear();

    let report = outcome.map(|_| state.report(puzzle));
    // Hand the caller's grid back even on failure, so it still sees how far we got.
    *grid = std::mem::take(&mut state.grid);
    report
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
) -> Vec<Vec<LineStatus>> {
    let lanes = puzzle.geometry.lane_map();
    (0..lanes.family_count())
        .map(|family| {
            lanes
                .family(family)
                .map(|lane| {
                    let mut gathered = vec![];
                    gather_into(lanes, lane, grid, &mut gathered);
                    analyze_line(&puzzle.lines[lane], ArrayView1::from(&gathered[..]))
                })
                .collect::<Vec<_>>()
        })
        .collect()
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

        let mut families = analyze_lines(&puzzle, &grid).into_iter();
        let row_tech = families.next().unwrap();
        let col_tech = families.next().unwrap();

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

        let bkg_solved = line_logic_solve(
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
        line_logic_solve(&puzzle, &mut None, &SolveOptions::default(), &mut grid).unwrap();

        for (cell, cell_filled) in grid.iter().zip(filled) {
            let truth = if *cell_filled { Color(1) } else { BACKGROUND };
            assert!(
                cell.can_be(truth),
                "solver ruled out the real color of a cell"
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
