use std::collections::{HashMap, HashSet};
use std::sync::mpsc;

use anyhow::Context;
use priority_queue::PriorityQueue;

use crate::{
    bt_solve::BtReport::{MultipleSolutions, UniqueSolution},
    geometry::GridKind,
    grid_solve::{LineCache, Report, SolveContext, SolveOptions, SolveState},
    gui,
    line_solve::Cell,
    puzzle::{Clue, Color, PartialSolution, Puzzle},
};

mod pickers;

use pickers::pick_guess;
pub use pickers::{PickerKind, PickerMix};

/// A coordinate into the tree of hypotheticals (a stack of assumptions)
#[derive(PartialEq, Eq, Hash, Debug, Clone)]
struct HypoCoord {
    guesses: Vec<(usize, Color)>,
}
// TODO: we should really impl push/pop on `HypoCoord`

pub enum BtReport {
    MultipleSolutions(), // TODO: provide *some* information
    UniqueSolution(Report),
}

pub struct BtContext<'p, 'x, C: Clue, K: GridKind> {
    q: PriorityQueue<HypoCoord, std::cmp::Reverse<Score>>,
    possibilities: HashMap<HypoCoord, BtSolveState<'p, C>>,
    linear_ctx: SolveContext<'p, 'x, C, K>,
    puzzle: &'p Puzzle<C, K>,
    solution_found: Option<PartialSolution>, // though we know it'll be a complete solution
    /// How many cells the puzzle has in total, for progress reporting
    total_cells: usize,
    /// Externally report progress
    progress: mpsc::Sender<f32>,
}

#[derive(Clone)]
struct BtSolveState<'p, C: Clue> {
    knowledge: SolveState<'p, C>,
    guesses_explored: HashSet<(usize, Color)>,
}

impl<'p, C: Clue> BtSolveState<'p, C> {
    /// How much is a node worth searching. Strong penalty for hypothetical depth:
    /// the goal is to prove a unique solution if possible, and that requires facts
    /// to filter down to "ground level"
    ///
    /// `solution_found` is the solution the search has already turned up, if it has: a node
    /// whose first guess agrees with it is on its way to rediscovering it, and rediscovering it
    /// settles nothing. What's wanted is a *second* solution, or the proof that there isn't one.
    fn score_at(
        &self,
        coord: &HypoCoord,
        solution_found: Option<&PartialSolution>,
    ) -> std::cmp::Reverse<Score> {
        let distance_remaining = self.knowledge.cells_left as f32;

        // Doubling per level, but only down to depth 5 — past there every node is buried anyway,
        // and what's left to do is order them, which a gentle slope does as well as a cliff.
        let levels = coord.guesses.len();
        let depth = 5.0 * 2.0_f32.powi(levels.min(5) as i32)
            + if levels > 5 { levels as f32 * 5.0 } else { 0.0 };

        let rediscovery = match (solution_found, coord.guesses.first()) {
            (Some(solution), Some((cell_idx, color))) if solution[*cell_idx].can_be(*color) => {
                500.0
            }
            _ => 0.0,
        };

        let exhaustion = self.guesses_explored.len() as f32 * 3.0;

        std::cmp::Reverse(Score(distance_remaining + depth + rediscovery + exhaustion))
    }

    fn fork(
        &mut self,
        coord: &HypoCoord,
        guess: (usize, Color),
    ) -> (HypoCoord, BtSolveState<'p, C>) {
        let mut new_coord = coord.clone();
        new_coord.guesses.push(guess);

        self.guesses_explored.insert(guess);
        (
            new_coord,
            BtSolveState {
                knowledge: self.knowledge.clone(),
                guesses_explored: HashSet::new(),
            },
        )
    }
}

/// Lower is better! This is used both to score nodes (`score_at`) and to score possible guesses
/// inside nodes (`pickers::GuessPicker::rate`)!
#[derive(PartialEq, PartialOrd, Debug)]
struct Score(f32);

impl Eq for Score {}

impl Ord for Score {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.partial_cmp(other).unwrap()
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

/// Learn that (assuming `coord`) the cell at `cell_idx` is [not] `color`.
/// When making a guess, `is` must be `true`, and `(cell_idx, color)` should be at the end of `coord`.
/// Errors on top-level contradiction. (Note that if `.run_and_check` is an error, we pop a guess and recur!)
/// Returns `None` if the search is still incomplete
fn suppose<'p, 'x, C: Clue, K: GridKind>(
    coord: &HypoCoord,
    cell_idx: usize,
    is: bool,
    color: Color,
    ctx: &mut BtContext<'p, 'x, C, K>,
) -> anyhow::Result<Option<BtReport>> {
    let state = ctx.possibilities.get_mut(&coord).unwrap();

    state
        .knowledge
        .learn(&mut ctx.linear_ctx, cell_idx, is, color);
    let run_consequence = state.knowledge.run_and_check(&mut ctx.linear_ctx);

    if ctx.linear_ctx.options.trace_backtrack {
        println!(
            "Rescoring {coord:?} to {:?}. Q len {}",
            state.score_at(coord, ctx.solution_found.as_ref()),
            ctx.q.len()
        );
    }
    // Did all that learning make this node look better?
    ctx.q
        .change_priority(coord, state.score_at(coord, ctx.solution_found.as_ref()));

    // `coord` is the root exactly when what was just learned is unconditional (no hypothesis
    // behind it), which is the only kind of progress worth reporting: hypothetical knowledge can
    // still be thrown away by a later contradiction.
    if coord.guesses.is_empty() {
        let done = ctx.total_cells.saturating_sub(state.knowledge.cells_left);
        let _ = ctx.progress.send(done as f32 / ctx.total_cells as f32);
    }

    match run_consequence {
        Err(e) => {
            let mut higher_coord = coord.clone();
            if let Some((prev_cell_idx, prev_color)) = higher_coord.guesses.pop() {
                if ctx.linear_ctx.options.trace_backtrack {
                    println!("Assumption {coord:?} wasn't true! So {prev_cell_idx} isn't {color:?}")
                }

                // Perhaps we instead ought to (lazily?) apply our knowledge to our sibling's descenents?
                // ...but they also might not be very valuable any more.
                nuke_descendants(&higher_coord, false, ctx);

                return suppose(
                    &higher_coord,
                    prev_cell_idx,
                    /*is=*/ false,
                    prev_color,
                    ctx,
                );
            } else {
                // end of the line!
                return Err(e).context("after counterfactual deduction");
            }
        }
        Ok(_) => {
            if state.knowledge.cells_left == 0 {
                if ctx.linear_ctx.options.trace_backtrack {
                    println!("Found a solution, assuming {coord:?}");
                }

                if coord.guesses.is_empty() {
                    return Ok(Some(UniqueSolution(state.knowledge.report(ctx.puzzle))));
                }

                if let Some(old_solution) = &ctx.solution_found {
                    if old_solution != &state.knowledge.grid {
                        return Ok(Some(MultipleSolutions()));
                    }
                } else {
                    ctx.solution_found = Some(state.knowledge.grid.clone())
                }

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
) -> anyhow::Result<BtReport> {
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
        return Ok(UniqueSolution(init_linear_state.report(puzzle))); // No backtracking required!
    }
    let _ = ctx
        .progress
        .send((total_cells - init_linear_state.cells_left) as f32 / total_cells as f32);

    let root_coord = HypoCoord { guesses: vec![] };

    ctx.q
        .push(root_coord.clone(), std::cmp::Reverse(Score(0.0)));
    ctx.possibilities.insert(
        root_coord,
        BtSolveState {
            knowledge: init_linear_state,
            guesses_explored: HashSet::new(),
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

        let kind = ctx.linear_ctx.options.guess_picker.for_guess(guesses_made);
        guesses_made += 1;

        // If we've made every possible guess, don't re-enqueue the node.
        if let Some((cell_idx, color)) = pick_guess(kind, state, &ctx.linear_ctx) {
            ctx.q.push(
                coord.clone(),
                state.score_at(&coord, ctx.solution_found.as_ref()),
            );

            let (new_coord, new_state) = state.fork(&coord, (cell_idx, color));

            if ctx.linear_ctx.options.trace_backtrack {
                println!("Guessing {new_coord:?}.")
            }

            // `suppose` expects to find `new_state` at `new_coord`
            ctx.q.push(
                new_coord.clone(),
                new_state.score_at(&new_coord, ctx.solution_found.as_ref()),
            );
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
    use crate::line_solve::Cell;
    use crate::puzzle::{BACKGROUND, ClueStyle, ColorInfo, Nono, Solution};

    /// Runs `backtrack_solve` to completion on the current thread, ignoring progress and never
    /// terminating early — what every test here wants, since none of them are testing that.
    fn solve_sync<C: Clue, K: GridKind>(
        puzzle: &Puzzle<C, K>,
        options: &SolveOptions,
    ) -> anyhow::Result<BtReport> {
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
        match solve_sync(&puzzle, &options).unwrap() {
            UniqueSolution(report) => {
                assert_eq!(report.cells_left, 0);
                assert_eq!(rendered(&report, 7), want);
            }
            MultipleSolutions() => panic!("this puzzle has exactly one solution"),
        }
    }

    /// A hollow box: line logic alone finishes it, so the search should never start.
    #[test]
    fn a_line_solvable_puzzle_needs_no_search() {
        let want = ["#####", "#...#", "#...#", "#...#", "#####"];
        let puzzle = solution_to_puzzle(&picture(&want));

        match solve_sync(&puzzle, &SolveOptions::default()).unwrap() {
            UniqueSolution(report) => {
                assert_eq!(report.cells_left, 0);
                assert_eq!(rendered(&report, 5), want);
            }
            MultipleSolutions() => panic!("this puzzle has exactly one solution"),
        }
    }

    /// Line logic stalls on this one with 18 of its 25 cells unknown. One guess in the upper-left-hand corner
    /// is sufficient to solve it.
    #[test]
    fn a_puzzle_that_needs_a_guess() {
        let want = ["..###", "..#.#", "##...", "....#", ".##.."];
        let puzzle = solution_to_puzzle(&picture(&want));

        let mut grid = vec![Cell::new(&puzzle.palette); puzzle.geometry.cell_count()];
        let line_only = crate::grid_solve::line_logic_solve(
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
            match solve_sync(&puzzle, &options).unwrap() {
                UniqueSolution(report) => {
                    assert_eq!(report.cells_left, 0, "{picker:?}");
                    assert_eq!(rendered(&report, 5), want, "{picker:?}");
                }
                MultipleSolutions() => panic!("this puzzle has exactly one solution ({picker:?})"),
            }
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
        let line_only = crate::grid_solve::line_logic_solve(
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
            match solve_sync(&puzzle, &options).unwrap() {
                UniqueSolution(report) => {
                    assert_eq!(report.cells_left, 0, "{picker:?}");
                    assert_eq!(rendered(&report, 7), want, "{picker:?}");
                }
                MultipleSolutions() => panic!("this puzzle has exactly one solution ({picker:?})"),
            }
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

        match solve_sync(&puzzle, &SolveOptions::default()).unwrap() {
            UniqueSolution(report) => {
                assert_eq!(report.cells_left, 0);
                assert_eq!(rendered(&report, 5), want);
            }
            MultipleSolutions() => panic!("this puzzle has exactly one solution"),
        }
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
        let line_only = crate::grid_solve::line_logic_solve(
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
        let line_only = crate::grid_solve::line_logic_solve(
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
            match solve_sync(&puzzle, &options).unwrap() {
                UniqueSolution(report) => {
                    assert_eq!(report.cells_left, 0, "{picker:?}");
                    // The lanes are ragged, so `rendered`'s fixed-width rows don't apply; compare
                    // against the picture the clues came from instead.
                    assert_eq!(
                        report.solution.cells(),
                        tri_picture(&want).cells,
                        "{picker:?}"
                    );
                }
                MultipleSolutions() => panic!("this puzzle has exactly one solution ({picker:?})"),
            }
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

        match solve_sync(&puzzle, &SolveOptions::default()).unwrap() {
            MultipleSolutions() => (),
            UniqueSolution(_) => panic!("both diagonals fit these clues"),
        }
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
