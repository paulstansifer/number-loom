//! Guidance for someone partway through a puzzle: what they've got wrong, and where to look next.
//!
//! First, the answer (or as much of it as can be found), to find the mistakes. Then, with the
//! mistakes erased, the lanes that settle cells by themselves (each considered against the grid
//! as it is, not after the others have had their say). Failing that, a guess that line logic can
//! show to be wrong.

use std::collections::HashMap;
use std::sync::mpsc;
use std::time::Duration;

use number_loom::geometry::{CellIdx, GridKind, LaneIdx};
use number_loom::puzzle::{Clue, Color, PartialSolution, Puzzle};
use number_loom::solve::conprop::conprop_solve_in_background;
use number_loom::solve::conprop_picker::Picker;
use number_loom::solve::grid_solve::{
    self, LineCache, SolveContext, SolveOptions, SolveState, gather_into,
};
use number_loom::solve::line_solve::{Cell, exhaust_line};
use rand::{SeedableRng, rngs::StdRng};

/// How long to search for the answer, where line logic doesn't find all of it.
const SEARCH_TIME: Duration = Duration::from_secs(2);
/// How many guesses to try, when nothing else turns anything up.
const GUESSES: usize = 200;
/// How many of those that lead to a contradiction are enough.
const CONTRADICTIONS: usize = 5;

/// How the answer was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Solved {
    LineLogic,
    Search,
    /// More than one grid fits the clues, so only what they all agree on is known.
    Ambiguous,
    /// The search ran out of time, so only what line logic found is known.
    OutOfTime,
}

/// A lane that, by itself, settles some cells the person hasn't.
#[derive(Clone, Debug)]
pub struct LineHint {
    pub lane: LaneIdx,
    /// Each cell, and what it has to be.
    pub resolves: Vec<(CellIdx, Color)>,
}

#[derive(Clone, Debug)]
pub enum Next {
    /// Every cell is decided (and none wrongly).
    Done,
    /// Lanes that settle cells by themselves, the one that settles the most first.
    Lines(Vec<LineHint>),
    /// (Only for color puzzles.) No lane settles a cell by itself, but between them, they rule
    /// out some colors. With those ruled out (perhaps over a few rounds), these lanes settle
    /// cells.
    LinesAfterPartial(Vec<LineHint>),
    /// For each, supposing the cell is the color leads (by line logic) to a contradiction. In
    /// the order the search would have guessed them.
    Contradictions(Vec<(CellIdx, Color)>),
    /// None of this many guesses led anywhere.
    Stuck { guesses: usize },
    /// These lanes can't be completed as they stand. (Mistakes are only caught where the answer
    /// is known, so this is possible when it isn't all known.)
    Broken(Vec<LaneIdx>),
}

#[derive(Clone, Debug)]
pub struct GuidanceReport {
    pub solved: Solved,
    /// What's known of the answer: all of it, unless `solved` says otherwise.
    pub answer: PartialSolution,
    /// The cells the person has got wrong.
    pub errors: Vec<CellIdx>,
    /// Where to go from here, once the errors are erased.
    pub next: Next,
}

/// Guidance for someone who's gotten as far as `grid` in `puzzle`. An error if the clues have no
/// solution.
pub fn guidance<C: Clue, K: GridKind>(
    puzzle: &Puzzle<C, K>,
    grid: &PartialSolution,
) -> anyhow::Result<GuidanceReport> {
    let (answer, solved) = find_answer(puzzle)?;

    let mut grid = grid.clone();
    let mut errors = vec![];
    for (cell, mine) in grid.iter_mut_enumerated() {
        // (A mistake is ruling out the answer.)
        let mut both = *mine;
        if both.learn_intersect(answer[cell]).is_err() {
            errors.push(cell);
            *mine = Cell::new(&puzzle.palette);
        }
    }

    let next = if grid.iter().all(Cell::is_known) {
        Next::Done
    } else {
        next_step(puzzle, grid)?
    };
    Ok(GuidanceReport {
        solved,
        answer,
        errors,
        next,
    })
}

/// Line logic, and then, if that doesn't finish the job, a search (for a while).
fn find_answer<C: Clue, K: GridKind>(
    puzzle: &Puzzle<C, K>,
) -> anyhow::Result<(PartialSolution, Solved)> {
    let report = grid_solve::solve(puzzle, &mut None, &SolveOptions::default())?;
    let line_logic = report.solution.to_partial();
    if report.cells_left == 0 {
        return Ok((line_logic, Solved::LineLogic));
    }

    // The search checks for a message on `terminate` every so often: send it one when time's up.
    let (terminate_s, terminate_r) = mpsc::channel();
    let (done_s, done_r) = mpsc::channel::<()>();
    let timer = std::thread::spawn(move || {
        let out_of_time = done_r.recv_timeout(SEARCH_TIME).is_err();
        if out_of_time {
            let _ = terminate_s.send(());
        }
        out_of_time
    });
    let searched = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(conprop_solve_in_background(
            puzzle,
            &SolveOptions::default(),
            mpsc::channel().0, // (No one's watching its progress.)
            terminate_r,
        ));
    let _ = done_s.send(());
    let out_of_time = timer.join().expect("the timer panicked");

    match searched {
        Ok(report) if report.cells_left == 0 => Ok((report.solution.to_partial(), Solved::Search)),
        // (When it finds a second solution, it reports what's known without guessing.)
        Ok(report) => Ok((report.solution.to_partial(), Solved::Ambiguous)),
        Err(_) if out_of_time => Ok((line_logic, Solved::OutOfTime)),
        Err(e) => Err(e),
    }
}

/// What every lane says about `grid`, each by itself.
struct Scrubbed {
    /// Lanes that settle cells, the one that settles the most first.
    hints: Vec<LineHint>,
    /// `grid`, with everything every lane rules out ruled out.
    narrowed: PartialSolution,
}

/// `Err` with the lanes that can't be completed (or that disagree with each other).
fn scrub_lanes<C: Clue, K: GridKind>(
    puzzle: &Puzzle<C, K>,
    grid: &PartialSolution,
) -> Result<Scrubbed, Vec<LaneIdx>> {
    let lane_map = &puzzle.geometry.lane_map;
    let mut narrowed = grid.clone();
    let (mut hints, mut broken) = (vec![], vec![]);
    let mut cells = vec![];
    for (lane, clues) in puzzle.lines.iter_enumerated() {
        gather_into(lane_map, lane, grid, &mut cells);
        let before = cells.clone();
        if exhaust_line(clues, &mut cells).is_err() {
            broken.push(lane);
            continue;
        }
        let mut resolves = vec![];
        for ((&cell, old), new) in lane_map.lanes[lane].cells.iter().zip(&before).zip(&cells) {
            if !old.is_known() && new.is_known() {
                resolves.push((cell, new.unwrap_color()));
            }
            if narrowed[cell].learn_intersect(*new).is_err() {
                broken.push(lane);
            }
        }
        if !resolves.is_empty() {
            hints.push(LineHint { lane, resolves });
        }
    }
    if !broken.is_empty() {
        broken.dedup();
        return Err(broken);
    }
    hints.sort_by_key(|hint| std::cmp::Reverse(hint.resolves.len()));
    Ok(Scrubbed { hints, narrowed })
}

fn next_step<C: Clue, K: GridKind>(
    puzzle: &Puzzle<C, K>,
    mut grid: PartialSolution,
) -> anyhow::Result<Next> {
    // (In black and white, ruling out a color settles the cell, so there's nothing partial.)
    let in_color = puzzle.palette.len() > 2;
    let mut partial = false;
    loop {
        let scrubbed = match scrub_lanes(puzzle, &grid) {
            Ok(scrubbed) => scrubbed,
            Err(broken) => return Ok(Next::Broken(broken)),
        };
        if !scrubbed.hints.is_empty() {
            return Ok(if partial {
                Next::LinesAfterPartial(scrubbed.hints)
            } else {
                Next::Lines(scrubbed.hints)
            });
        }
        if !in_color || scrubbed.narrowed == grid {
            break;
        }
        grid = scrubbed.narrowed;
        partial = true;
    }
    guess(puzzle, grid)
}

/// Try the guesses the search would make first, until enough lead to contradictions.
fn guess<C: Clue, K: GridKind>(
    puzzle: &Puzzle<C, K>,
    grid: PartialSolution,
) -> anyhow::Result<Next> {
    let mut line_cache = Some(LineCache::new());
    let options = SolveOptions::default();
    let mut ctx = SolveContext::new(puzzle, &mut line_cache, &options);
    let mut stalled = SolveState::new(&mut ctx, grid);
    // Line logic has nothing to add (that's why we're guessing), but with this out of the way,
    // each guess only has to revisit the lanes it bears on.
    stalled.run_and_check(&mut ctx)?;

    let no_history = HashMap::new();
    let mut picker = Picker::from_situation(
        puzzle,
        &stalled.grid,
        &no_history,
        &mut StdRng::seed_from_u64(0),
    );
    let mut contradictions = vec![];
    let mut tried = 0;
    while tried < GUESSES && contradictions.len() < CONTRADICTIONS {
        let Some((cell, color)) = picker.pick(puzzle, &stalled.grid, &no_history) else {
            break;
        };
        tried += 1;
        let mut supposing = stalled.clone();
        if supposing.learn(&mut ctx, cell, true, color).is_err()
            || supposing.run_and_check(&mut ctx).is_err()
        {
            contradictions.push((cell, color));
        }
    }
    Ok(if contradictions.is_empty() {
        Next::Stuck { guesses: tried }
    } else {
        Next::Contradictions(contradictions)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use number_loom::geometry::Square;
    use number_loom::import::bw_palette;
    use number_loom::puzzle::{BACKGROUND, Nono};

    const BLACK: Color = Color(1);

    /// The puzzle whose answer is `rows` (`#` for black, `.` for white).
    fn puzzle(rows: &[&str]) -> Puzzle<Nono, Square> {
        let grid: Vec<Vec<bool>> = rows
            .iter()
            .map(|r| r.chars().map(|c| c == '#').collect())
            .collect();
        let clues = |line: Vec<bool>| -> Vec<Nono> {
            line.chunk_by(|a, b| a == b)
                .filter(|run| run[0])
                .map(|run| Nono {
                    color: BLACK,
                    count: run.len() as u16,
                })
                .collect()
        };
        let width = grid[0].len();
        Puzzle::square(
            bw_palette(),
            grid.iter().map(|row| clues(row.clone())).collect(),
            (0..width)
                .map(|x| clues(grid.iter().map(|row| row[x]).collect()))
                .collect(),
        )
    }

    /// `#` filled in, `x` crossed out, `.` undecided.
    fn progress(puzzle: &Puzzle<Nono, Square>, rows: &[&str]) -> PartialSolution {
        rows.iter()
            .flat_map(|r| r.chars())
            .map(|c| match c {
                '#' => Cell::from_color(BLACK),
                'x' => Cell::from_color(BACKGROUND),
                _ => Cell::new(&puzzle.palette),
            })
            .collect()
    }

    fn cell(puzzle: &Puzzle<Nono, Square>, x: usize, y: usize) -> CellIdx {
        puzzle.geometry.cell((x, y)).unwrap()
    }

    #[test]
    fn lanes_that_settle_the_most_come_first() {
        let p = puzzle(&["#####", "#....", "#.###", "#...#", "#####"]);
        let report = guidance(
            &p,
            &progress(&p, &[".....", ".....", ".....", ".....", "....."]),
        )
        .unwrap();
        assert_eq!(report.solved, Solved::LineLogic);
        assert!(report.errors.is_empty());
        let Next::Lines(hints) = report.next else {
            panic!("{:?}", report.next);
        };
        // (Several lanes settle all five.)
        assert_eq!(hints[0].resolves.len(), 5);
        assert_eq!(hints[1].resolves.len(), 5);
        assert!(
            hints
                .windows(2)
                .all(|w| w[0].resolves.len() >= w[1].resolves.len())
        );
    }

    #[test]
    fn mistakes_are_found_and_erased() {
        let p = puzzle(&["#####", "#....", "#.###", "#...#", "#####"]);
        let report = guidance(
            &p,
            &progress(&p, &["#####", "x#xxx", "#x###", "#xxx#", "#####"]),
        )
        .unwrap();
        assert_eq!(report.errors, vec![cell(&p, 0, 1), cell(&p, 1, 1)]);
        let Next::Lines(hints) = report.next else {
            panic!("{:?}", report.next);
        };
        // Row 2 ("1") could be either of them, but each column settles its own.
        let mut resolves: Vec<_> = hints.iter().flat_map(|h| h.resolves.clone()).collect();
        resolves.sort_by_key(|(cell, _)| *cell);
        assert_eq!(
            resolves,
            vec![(cell(&p, 0, 1), BLACK), (cell(&p, 1, 1), BACKGROUND)]
        );
    }

    #[test]
    fn finished() {
        let p = puzzle(&["##.", ".#.", ".##"]);
        let report = guidance(&p, &progress(&p, &["##x", "x#x", "x##"])).unwrap();
        assert!(report.errors.is_empty());
        assert!(matches!(report.next, Next::Done), "{:?}", report.next);
    }

    #[test]
    fn guesses_that_go_wrong() {
        let p = puzzle(&[
            "###..#.", ".....#.", ".####..", "...#..#", ".##...#", "....##.", "..####.",
        ]);
        // As far as line logic gets.
        let grid = progress(
            &p,
            &[
                "###xx..", "xxxxx..", "x.###.x", "xxx#x..", "x##xx..", "xxxx##x", "x.###.x",
            ],
        );
        let report = guidance(&p, &grid).unwrap();
        assert_eq!(report.solved, Solved::Search);
        let Next::Contradictions(contradictions) = report.next else {
            panic!("{:?}", report.next);
        };
        assert!((1..=CONTRADICTIONS).contains(&contradictions.len()));
        for (cell, color) in contradictions {
            assert!(!grid[cell].is_known());
            assert!(!report.answer[cell].can_be(color));
        }
    }

    #[test]
    fn nothing_to_go_on_in_an_ambiguous_puzzle() {
        // Either diagonal fits.
        let p = puzzle(&["#.", ".#"]);
        let report = guidance(&p, &progress(&p, &["..", ".."])).unwrap();
        assert_eq!(report.solved, Solved::Ambiguous);
        assert!(report.errors.is_empty());
        assert!(
            matches!(report.next, Next::Stuck { .. }),
            "{:?}",
            report.next
        );
    }

    #[test]
    fn contradictory_clues() {
        // The rows fill two cells; the columns, one.
        let p = Puzzle::square(
            bw_palette(),
            vec![
                vec![Nono {
                    color: BLACK,
                    count: 2,
                }],
                vec![],
            ],
            vec![
                vec![Nono {
                    color: BLACK,
                    count: 1,
                }],
                vec![],
            ],
        );
        assert!(guidance(&p, &progress(&p, &["..", ".."])).is_err());
    }
}
