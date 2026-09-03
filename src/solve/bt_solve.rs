use std::collections::{BTreeSet, HashMap};
use std::sync::mpsc;

use anyhow::Context;
use priority_queue::PriorityQueue;

use crate::{
    geometry::GridKind,
    gui,
    puzzle::{Clue, Color, PartialSolution, Puzzle},
    solve::grid_solve::{LineCache, Report, SolveContext, SolveOptions, SolveState},
    solve::line_solve::{Cell, ModeMap, SolveMode},
};

#[path = "bt_picking.rs"]
mod bt_picking;
#[path = "bt_scoring.rs"]
mod bt_scoring;

use bt_picking::pick_guess;
pub use bt_picking::{PickerKind, PickerMix};
use bt_scoring::{ScoreCtx, Terms};
pub use bt_scoring::{ScoreKind, ScorerPair};

/// A coordinate into the tree of hypotheticals (a stack of assumptions)
#[derive(PartialEq, Eq, Hash, Debug, Clone)]
struct HypoCoord {
    guesses: Vec<(usize, Color)>,
}
// TODO: we should really impl push/pop on `HypoCoord`

/// The root's own coordinate: no hypothesis at all, so its knowledge is unconditional.
fn root_coord() -> HypoCoord {
    HypoCoord { guesses: vec![] }
}

pub struct BtContext<'p, 'x, C: Clue, K: GridKind> {
    q: PriorityQueue<HypoCoord, std::cmp::Reverse<Score>>,
    possibilities: HashMap<HypoCoord, BtSolveState>,
    linear_ctx: SolveContext<'p, 'x, C, K>,
    puzzle: &'p Puzzle<C, K>,
    solution_found: Option<PartialSolution>, // though we know it'll be a complete solution
    /// How many cells the puzzle has in total, for progress reporting
    total_cells: usize,
    /// Externally report progress
    progress: mpsc::Sender<f32>,
}

#[derive(Clone)]
struct BtSolveState {
    /// It's cheaper to do `SolveState::resume` each time than to keep `SolveState`.
    grid: PartialSolution,
    /// We *could* add it to the grid immediately instead, but we'd need to track what coord is dirty
    extra_knowledge: Vec<(usize, bool, Color)>,
    cells_left: usize,
    /// Keep the history of steps taken (TODO: this isn't that meaningful anyways.)
    solve_counts: ModeMap<usize>,
    guesses_explored: BTreeSet<(usize, Color)>,
    /// How many cells were unknown in the node this one was forked from, so a scorer can ask
    /// what this node's guess actually bought. The root is its own parent.
    parent_cells_left: usize,
}

impl BtSolveState {
    /// How much is a node worth searching: lower is sooner. The formulas live in
    /// `bt_scoring.rs`; this gathers the measurements they read.
    fn score_at(&self, coord: &HypoCoord, sc: &ScoreCtx<'_>) -> std::cmp::Reverse<Score> {
        let rediscovering = match (sc.solution_found, coord.guesses.first()) {
            (Some(solution), Some((cell_idx, color))) => solution[*cell_idx].can_be(*color),
            _ => false,
        };

        let terms = Terms {
            cells_left: self.cells_left as f32,
            parent_cells_left: self.parent_cells_left as f32,
            candidates: self.candidates() as f32,
            levels: coord.guesses.len() as f32,
            explored: self.guesses_explored.len() as f32,
            rediscovering,
            total_cells: sc.total_cells as f32,
            solution_known: sc.solution_found.is_some(),
        };

        std::cmp::Reverse(Score(bt_scoring::score(sc.kind, &terms)))
    }

    /// Summed candidate colors over the unknown cells — only the `Candidates` scorer asks, and
    /// it's a pass over the grid, which is what `pick_guess` costs anyway.
    fn candidates(&self) -> usize {
        self.grid
            .iter()
            .map(|cell| cell.raw().count_ones() as usize)
            .sum()
    }

    fn fork(&mut self, coord: &HypoCoord, guess: (usize, Color)) -> (HypoCoord, BtSolveState) {
        let mut new_coord = coord.clone();
        new_coord.guesses.push(guess);

        self.guesses_explored.insert(guess);
        (
            new_coord,
            BtSolveState {
                grid: self.grid.clone(),
                extra_knowledge: self.extra_knowledge.clone(),
                cells_left: self.cells_left,
                solve_counts: self.solve_counts,
                guesses_explored: BTreeSet::new(),
                parent_cells_left: self.cells_left,
            },
        )
    }
}

/// Lower is better! This is used both to score nodes (`score_at`) and to score possible guesses
/// inside nodes (`bt_picking::GuessPicker::rate`)!
#[derive(PartialEq, Debug)]
struct Score(f32);

impl Eq for Score {}

impl Ord for Score {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.partial_cmp(&other.0).expect("a score was NaN")
    }
}

impl PartialOrd for Score {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

fn nuke_node<'p, 'x, C: Clue, K: GridKind>(coord: &HypoCoord, ctx: &mut BtContext<'p, 'x, C, K>) {
    nuke_descendants(coord, false, ctx); // first descend...

    // Because of tombstones (I think ...), these may not be present
    ctx.q.remove(coord); // ...now it's no longer needed
    ctx.possibilities.remove(coord);
    // The parent may still have an entry in `guesses_explored` as a tombstone!
}

fn nuke_descendants<'p, 'x, C: Clue, K: GridKind>(
    coord: &HypoCoord,
    keep_tombstones: bool,
    ctx: &mut BtContext<'p, 'x, C, K>,
) {
    // Skip tombstones:
    if let Some(state) = ctx.possibilities.get(&coord) {
        let guesses_here = state.guesses_explored.clone();
        for guess in guesses_here {
            let mut new_coord = coord.clone();
            new_coord.guesses.push(guess);

            nuke_node(&new_coord, ctx);
        }

        if !keep_tombstones {
            ctx.possibilities
                .get_mut(coord)
                .unwrap()
                .guesses_explored
                .clear();
        }
    }
}

fn inform_descendents<'p, 'x, C: Clue, K: GridKind>(
    coord: &HypoCoord,
    (cell_idx, is, color): (usize, bool, Color),
    ctx: &mut BtContext<'p, 'x, C, K>,
) {
    if let Some(state) = ctx.possibilities.get(&coord) {
        let guesses_here = state.guesses_explored.clone();
        for guess in guesses_here {
            let mut new_coord = coord.clone();
            new_coord.guesses.push(guess);

            if let Some(new_state) = ctx.possibilities.get_mut(&new_coord) {
                new_state.extra_knowledge.push((cell_idx, is, color));
            }

            inform_descendents(&new_coord, (cell_idx, is, color), ctx);
        }
    }
}

/// Learn that (assuming `coord`) the cell at `cell_idx` is [not] `color`.
/// When making a guess, `is` must be `true`, and `(cell_idx, color)` should be at the end of `coord`.
/// (We could store `(cell_idx, is, color)` instead, but I think there's not much call for that)
/// Errors on top-level contradiction. (Note that if `.run_and_check` is an error, we pop a guess and recur!)
/// Returns `None` if the search is still incomplete
fn suppose<'p, 'x, C: Clue, K: GridKind>(
    coord: &HypoCoord,
    cell_idx: usize,
    is: bool,
    color: Color,
    ctx: &mut BtContext<'p, 'x, C, K>,
) -> anyhow::Result<Option<Report>> {
    let state = ctx.possibilities.get_mut(coord).unwrap();

    // Rebuild the line-logic bookkeeping. `mem::take` leaves an empty grid behind, but we will put it back.
    let grid = std::mem::take(&mut state.grid);
    let mut working = SolveState::resume(&mut ctx.linear_ctx, grid);

    // Reapply what we learned from our cousins; is it even consistent with us?
    let mut learned = Ok(());
    for &(cousin_idx, cousin_is, cousin_color) in &state.extra_knowledge {
        learned = working
            .learn(&mut ctx.linear_ctx, cousin_idx, cousin_is, cousin_color)
            .map(|_| ());
        if learned.is_err() {
            break; // the rest can't matter; this node is already impossible
        }
    }
    state.extra_knowledge.clear();
    if learned.is_ok() {
        // TODO: maybe fold into the above loop.
        learned = working
            .learn(&mut ctx.linear_ctx, cell_idx, is, color)
            .map(|_| ());
    }

    // No sense running line logic over a grid we already know can't be filled in.
    let run_consequence = match learned {
        Ok(()) => working.run_and_check(&mut ctx.linear_ctx),
        Err(e) => Err(e),
    };

    state.grid = working.grid;
    state.cells_left = working.cells_left;
    for mode in SolveMode::all() {
        state.solve_counts[*mode] += working.solve_counts[*mode];
    }

    let sc = ScoreCtx {
        kind: ctx.linear_ctx.options.node_scorer,
        total_cells: ctx.total_cells,
        solution_found: ctx.solution_found.as_ref(),
    };
    if ctx.linear_ctx.options.trace_backtrack {
        println!(
            "Rescoring {coord:?} to {:?}. Q len {}",
            state.score_at(coord, &sc),
            ctx.q.len()
        );
    }
    // Did all that learning make this node look better?
    // (usually, this is the same as .change_priority, but `coord` might have gotten nuked!)
    ctx.q.push(coord.clone(), state.score_at(coord, &sc));

    // `coord` is the root exactly when what was just learned is unconditional (no hypothesis
    // behind it), which is the only kind of progress worth reporting: hypothetical knowledge can
    // still be thrown away by a later contradiction.
    if coord.guesses.is_empty() {
        let done = ctx.total_cells.saturating_sub(state.cells_left);
        let _ = ctx.progress.send(done as f32 / ctx.total_cells as f32);
    }

    match run_consequence {
        Err(e) => {
            let mut higher_coord = coord.clone();
            if let Some((prev_cell_idx, prev_color)) = higher_coord.guesses.pop() {
                if ctx.linear_ctx.options.trace_backtrack {
                    println!("Assumption {coord:?} wasn't true! So {prev_cell_idx} isn't {color:?}")
                }

                nuke_node(coord, ctx); // Get rid of the contradiction node
                inform_descendents(&higher_coord, (prev_cell_idx, false, prev_color), ctx);

                return suppose(&higher_coord, prev_cell_idx, false, prev_color, ctx);
            } else {
                // end of the line!
                return Err(e).context("after counterfactual deduction");
            }
        }
        Ok(_) => {
            if state.cells_left == 0 {
                if ctx.linear_ctx.options.trace_backtrack {
                    println!("Found a solution, assuming {coord:?}");
                }

                if coord.guesses.is_empty() {
                    return Ok(Some(Report::from_grid(
                        ctx.puzzle,
                        &state.grid,
                        state.cells_left,
                        state.solve_counts,
                    )));
                }

                // Hunting only: the first complete grid is the answer, whether or not anything
                // else would also fit.
                if ctx.linear_ctx.options.stop_at_first_solution {
                    return Ok(Some(Report::from_grid(
                        ctx.puzzle,
                        &state.grid,
                        state.cells_left,
                        state.solve_counts,
                    )));
                }

                if let Some(old_solution) = &ctx.solution_found {
                    if old_solution != &state.grid {
                        // Time to give up! If we churned for longer, we might be able to
                        // reduce the number of unknown cells, but we know there will always be some.
                        let root = &ctx.possibilities[&root_coord()];
                        let root_report = Report::from_grid(
                            ctx.puzzle,
                            &root.grid,
                            root.cells_left,
                            root.solve_counts,
                        );
                        return Ok(Some(root_report));
                    }
                } else {
                    ctx.solution_found = Some(state.grid.clone())
                }

                // The parent of `coord` will retain an entry as a tombstone.
                nuke_node(coord, ctx);
            }
        }
    }

    Ok(None)
}

pub async fn backtrack_solve<C: Clue, K: GridKind>(
    puzzle: &Puzzle<C, K>,
    options: &SolveOptions,
    progress: mpsc::Sender<f32>,
    terminate: mpsc::Receiver<()>,
) -> anyhow::Result<Report> {
    let mut line_cache: Option<LineCache<C>> = Some(LineCache::new());

    let options = SolveOptions {
        display_cli_progress: false,
        ..options.clone()
    };
    let total_cells = puzzle.geometry.cell_count();
    let mut ctx = BtContext {
        q: PriorityQueue::new(),
        possibilities: HashMap::default(),
        linear_ctx: SolveContext::new(puzzle, &mut line_cache, &options),
        puzzle,
        solution_found: None,
        total_cells,
        progress,
    };

    let mut init_linear_state = SolveState::new(
        &mut ctx.linear_ctx,
        vec![Cell::new(&puzzle.palette); puzzle.geometry.cell_count()],
    );
    init_linear_state.run_and_check(&mut ctx.linear_ctx)?; // `?` because contradictions here are "real"

    if init_linear_state.cells_left == 0 {
        let _ = ctx.progress.send(1.0);
        return Ok(init_linear_state.report(puzzle)); // No backtracking required!
    }
    let _ = ctx
        .progress
        .send((total_cells - init_linear_state.cells_left) as f32 / total_cells as f32);

    ctx.q.push(root_coord(), std::cmp::Reverse(Score(0.0)));
    let root_cells_left = init_linear_state.cells_left;
    ctx.possibilities.insert(
        root_coord(),
        BtSolveState {
            grid: init_linear_state.grid,
            extra_knowledge: vec![],
            cells_left: root_cells_left,
            solve_counts: init_linear_state.solve_counts,
            guesses_explored: BTreeSet::new(),
            parent_cells_left: root_cells_left,
        },
    );

    // Counts guesses rather than nodes, so a mixed `PickerMix` rotates one picker per guess.
    let mut guesses_made = 0;

    while let Some((coord, _score)) = ctx.q.pop() {
        if terminate.try_recv().is_ok() {
            anyhow::bail!("backtracking search cancelled");
        }
        gui::yield_now().await;

        let state = ctx.possibilities.get_mut(&coord).unwrap();
        let sc = ScoreCtx {
            kind: ctx.linear_ctx.options.node_scorer,
            total_cells: ctx.total_cells,
            solution_found: ctx.solution_found.as_ref(),
        };

        let kind = ctx.linear_ctx.options.guess_picker.for_guess(guesses_made);
        guesses_made += 1;

        // If we've made every possible guess, don't re-enqueue the node.
        if let Some((cell_idx, color)) = pick_guess(kind, state, &ctx.linear_ctx) {
            ctx.q.push(coord.clone(), state.score_at(&coord, &sc));

            let (new_coord, new_state) = state.fork(&coord, (cell_idx, color));

            if ctx.linear_ctx.options.trace_backtrack {
                println!("Guessing {new_coord:?}.")
            }

            // `suppose` expects to find `new_state` at `new_coord`
            ctx.q
                .push(new_coord.clone(), new_state.score_at(&new_coord, &sc));
            ctx.possibilities.insert(new_coord.clone(), new_state);

            if let Some(sup_res) =
                suppose(&new_coord, cell_idx, /*is=*/ true, color, &mut ctx)?
            {
                let _ = ctx.progress.send(1.0);
                return Ok(sup_res); // We're done!
            }
        }
    }
    unreachable!("The root state shouldn't be removed without finding a unique solution");
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::geometry::{Geometry, Outline, Rect, Square, Tri};
    use crate::import::{bw_palette, solution_to_puzzle, solution_to_tri_puzzle};
    use crate::puzzle::{BACKGROUND, ClueStyle, ColorInfo, Nono, Solution};
    use crate::solve::line_solve::Cell;

    /// Runs `backtrack_solve` to completion on the current thread, ignoring progress and never
    /// terminating early — what every test here wants, since none of them are testing that.
    fn solve_sync<C: Clue, K: GridKind>(
        puzzle: &Puzzle<C, K>,
        options: &SolveOptions,
    ) -> anyhow::Result<Report> {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(backtrack_solve(
                puzzle,
                options,
                mpsc::channel().0,
                mpsc::channel().1,
            ))
    }

    /// A picture, written a row at a time: `.` is the background, and every other character is a
    /// foreground color, numbered in the order the characters first appear. The palette is built
    /// to match, so a two-character picture is black and white and a three-character one isn't.
    /// `Solution`'s cells are row-major, so the rows go in exactly as written.
    fn picture(rows: &[&str]) -> Solution<Square> {
        let width = rows[0].len();
        assert!(rows.iter().all(|r| r.len() == width), "ragged picture");

        let mut palette = HashMap::from([(BACKGROUND, ColorInfo::default_bg())]);
        let mut seen: Vec<char> = vec![];
        let cells = rows
            .iter()
            .flat_map(|row| row.chars())
            .map(|ch| {
                if ch == '.' {
                    return BACKGROUND;
                }
                let which = seen.iter().position(|c| *c == ch).unwrap_or_else(|| {
                    seen.push(ch);
                    seen.len() - 1
                });
                let color = Color(which as u8 + 1);
                palette.entry(color).or_insert(ColorInfo::default_fg(color));
                color
            })
            .collect();

        Solution::new(
            ClueStyle::Nono,
            palette,
            Geometry::new(Rect {
                width,
                height: rows.len(),
            }),
            cells,
        )
    }

    /// The 16-cell triddler from `webpbn_tridder.md`, in rows of 5, 6, and 5 — written the way
    /// `picture` writes a square one, since a triddler's cells are dense in row order too and so
    /// the rows simply concatenate. Black and white only; the point here is the shape.
    fn tri_picture(rows: &[&str; 3]) -> Solution<Tri> {
        let geometry = Geometry::<Tri>::new(Outline {
            a: (0, 2),
            b: (1, 3),
            c: (-1, 2),
        });
        let cells: Vec<Color> = rows
            .iter()
            .flat_map(|row| row.chars())
            .map(|ch| if ch == '.' { BACKGROUND } else { Color(1) })
            .collect();
        assert_eq!(
            cells.len(),
            geometry.cell_count(),
            "wrong number of cells for this outline"
        );
        Solution::new(ClueStyle::Nono, bw_palette(), geometry, cells)
    }

    /// One lane's worth of `Nono` clues, all in `Color(1)`, for the tests that write clues out
    /// directly instead of deriving them from a picture.
    fn runs(counts: &[u16]) -> Vec<Nono> {
        counts
            .iter()
            .map(|count| Nono {
                color: Color(1),
                count: *count,
            })
            .collect()
    }

    /// The picture a `UniqueSolution` report describes, rendered the way `picture` reads one —
    /// so a solved report can be compared straight against the rows that built the puzzle.
    fn rendered(report: &Report, width: usize) -> Vec<String> {
        report
            .solution
            .cells()
            .chunks(width)
            .map(|row| {
                row.iter()
                    .map(|c| ".#o".chars().nth(c.0 as usize).expect("too many colors"))
                    .collect()
            })
            .collect()
    }

    /// A mixed rotation has to solve puzzles too — including one deep enough that the rotation
    /// actually turns over.
    #[test]
    fn a_mixed_rotation_solves_a_puzzle() {
        let want = [
            "...##..", ".#.#...", "##..##.", "..##.##", "##.....", "#..#..#", ".##.#.#",
        ];
        let puzzle = solution_to_puzzle(&picture(&want));

        let options = SolveOptions {
            guess_picker: "disagreement:3,random:1".parse().unwrap(),
            ..SolveOptions::default()
        };
        let report = solve_sync(&puzzle, &options).unwrap();
        assert_eq!(report.cells_left, 0, "this puzzle has exactly one solution");
        assert_eq!(rendered(&report, 7), want);
    }

    /// A hollow box: line logic alone finishes it, so the search should never start.
    #[test]
    fn a_line_solvable_puzzle_needs_no_search() {
        let want = ["#####", "#...#", "#...#", "#...#", "#####"];
        let puzzle = solution_to_puzzle(&picture(&want));

        let report = solve_sync(&puzzle, &SolveOptions::default()).unwrap();
        assert_eq!(report.cells_left, 0, "this puzzle has exactly one solution");
        assert_eq!(rendered(&report, 5), want);
    }

    /// Line logic stalls on this one with 18 of its 25 cells unknown. One guess in the upper-left-hand corner
    /// is sufficient to solve it.
    #[test]
    fn a_puzzle_that_needs_a_guess() {
        let want = ["..###", "..#.#", "##...", "....#", ".##.."];
        let puzzle = solution_to_puzzle(&picture(&want));

        let mut grid = vec![Cell::new(&puzzle.palette); puzzle.geometry.cell_count()];
        let line_only = crate::solve::grid_solve::line_logic_solve(
            &puzzle,
            &mut None,
            &SolveOptions::default(),
            &mut grid,
        )
        .unwrap();
        assert_eq!(line_only.cells_left, 18); // Line logic stalled!

        for picker in PickerKind::ALL {
            let options = SolveOptions {
                guess_picker: PickerMix::single(picker),
                ..SolveOptions::default()
            };
            let report = solve_sync(&puzzle, &options).unwrap();
            assert_eq!(
                report.cells_left, 0,
                "this puzzle has exactly one solution ({picker:?})"
            );
            assert_eq!(rendered(&report, 5), want, "{picker:?}");
        }
    }

    /// Bigger, and stalled harder: line logic gets 17 of 49 cells and the rest have to be
    /// guessed, so the search has to go several levels deep and back out again rather than
    /// getting there on one lucky assumption.
    #[test]
    fn a_puzzle_that_needs_several_guesses() {
        let want = [
            "...##..", ".#.#...", "##..##.", "..##.##", "##.....", "#..#..#", ".##.#.#",
        ];
        let puzzle = solution_to_puzzle(&picture(&want));

        let mut grid = vec![Cell::new(&puzzle.palette); puzzle.geometry.cell_count()];
        let line_only = crate::solve::grid_solve::line_logic_solve(
            &puzzle,
            &mut None,
            &SolveOptions::default(),
            &mut grid,
        )
        .unwrap();
        assert_eq!(line_only.cells_left, 32);

        for picker in PickerKind::ALL {
            let options = SolveOptions {
                guess_picker: PickerMix::single(picker),
                ..SolveOptions::default()
            };
            let report = solve_sync(&puzzle, &options).unwrap();
            assert_eq!(
                report.cells_left, 0,
                "this puzzle has exactly one solution ({picker:?})"
            );
            assert_eq!(rendered(&report, 7), want, "{picker:?}");
        }
    }

    /// Three colors, so ruling a cell out doesn't settle it: `suppose`'s `is = false` has to
    /// leave two possibilities standing where a black-and-white puzzle would be left with one.
    #[test]
    fn a_multicolor_puzzle_that_needs_a_guess() {
        let want = [".##..", "oo.#.", "o..#o", "o...o", "##..#"];
        let puzzle = solution_to_puzzle(&picture(&want));
        assert_eq!(
            puzzle.palette.len(),
            3,
            "background and two foreground colors"
        );

        let report = solve_sync(&puzzle, &SolveOptions::default()).unwrap();
        assert_eq!(report.cells_left, 0, "this puzzle has exactly one solution");
        assert_eq!(rendered(&report, 5), want);
    }

    /// Clues with no picture behind them at all — but every lane is satisfiable on its own, and
    /// the row and column totals even agree, so line logic runs out of things to say with 16
    /// cells still unknown rather than reporting a contradiction. Only the search can find out,
    /// which means the error has to survive `suppose` unwinding every guess back to the root.
    #[test]
    fn a_puzzle_with_no_solution_is_an_error() {
        let puzzle = Puzzle::square(
            bw_palette(),
            vec![runs(&[]), runs(&[1]), runs(&[2]), runs(&[2]), runs(&[2])],
            vec![
                runs(&[2]),
                runs(&[1, 1]),
                runs(&[1, 1]),
                runs(&[1]),
                runs(&[]),
            ],
        );

        // Line logic really doesn't notice; if it learns to, this stops testing the search.
        let mut grid = vec![Cell::new(&puzzle.palette); puzzle.geometry.cell_count()];
        let line_only = crate::solve::grid_solve::line_logic_solve(
            &puzzle,
            &mut None,
            &SolveOptions::default(),
            &mut grid,
        )
        .unwrap();
        assert_eq!(line_only.cells_left, 16);

        assert!(solve_sync(&puzzle, &SolveOptions::default()).is_err());
    }

    /// `backtrack_solve` is generic over the grid shape, and a triddler is the part of that
    /// generality a square puzzle can't reach: three clue directions instead of two, and lanes
    /// of differing lengths that meet in places no row-and-column shortcut would predict. Line
    /// logic gets 6 of these 16 cells and stops.
    #[test]
    fn a_triddler_that_needs_a_guess() {
        let want = [".#..#", "..##..", "....."];
        let puzzle = solution_to_tri_puzzle(&tri_picture(&want));

        let mut grid = vec![Cell::new(&puzzle.palette); puzzle.geometry.cell_count()];
        let line_only = crate::solve::grid_solve::line_logic_solve(
            &puzzle,
            &mut None,
            &SolveOptions::default(),
            &mut grid,
        )
        .unwrap();
        assert_eq!(line_only.cells_left, 10);

        for picker in PickerKind::ALL {
            let options = SolveOptions {
                guess_picker: PickerMix::single(picker),
                ..SolveOptions::default()
            };
            let report = solve_sync(&puzzle, &options).unwrap();
            assert_eq!(
                report.cells_left, 0,
                "this puzzle has exactly one solution ({picker:?})"
            );
            // The lanes are ragged, so `rendered`'s fixed-width rows don't apply; compare
            // against the picture the clues came from instead.
            assert_eq!(
                report.solution.cells(),
                tri_picture(&want).cells,
                "{picker:?}"
            );
        }
    }

    /// Column 0 wants one filled cell and gets none. What makes this worth its own test is
    /// where the mistake comes from: line logic fills all four cells from the rows, sees
    /// `cells_left` reach zero, and reports success without ever looking at the columns — so the
    /// contradiction is one only the check on the way out can catch.
    #[test]
    fn a_grid_that_only_looks_solved_is_an_error() {
        let puzzle = Puzzle::square(
            bw_palette(),
            vec![runs(&[1]), runs(&[1])],
            vec![runs(&[1]), runs(&[2])],
        );

        assert!(solve_sync(&puzzle, &SolveOptions::default()).is_err());
    }

    /// One filled cell per row and per column of a 2x2 grid: the two diagonals both fit.
    #[test]
    fn an_ambiguous_puzzle_reports_multiple_solutions() {
        let puzzle = solution_to_puzzle(&picture(&["#.", ".#"]));

        let report = solve_sync(&puzzle, &SolveOptions::default()).unwrap();
        assert!(report.cells_left > 0, "both diagonals fit these clues");
    }

    /// A run that doesn't fit in the lane it's a clue for. Line logic sees the contradiction on
    /// its first pass, before the search ever starts, so it has to arrive as an error rather than
    /// as a report of a puzzle with no solutions.
    #[test]
    fn impossible_clues_are_an_error() {
        let clue = |count| {
            vec![Nono {
                color: Color(1),
                count,
            }]
        };
        // Two columns, so the first row's run of three has nowhere to go.
        let puzzle = Puzzle::square(bw_palette(), vec![clue(3), clue(1)], vec![clue(1), clue(1)]);

        assert!(solve_sync(&puzzle, &SolveOptions::default()).is_err());
    }
}
