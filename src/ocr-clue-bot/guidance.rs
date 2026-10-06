//! Guidance for someone partway through a puzzle: what they've got wrong, and where to look next.
//!
//! First, the answer (or as much of it as can be found), to find the mistakes. Then, with the
//! mistakes erased, the lanes that settle cells by themselves (each considered against the grid
//! as it is, not after the others have had their say). Failing that, a guess that line logic can
//! show to be wrong.

use std::collections::HashMap;
use std::sync::mpsc;
use std::time::Duration;

use number_loom::formats::partial_cells::has_progress;
use number_loom::formats::woven::to_woven;
use number_loom::geometry::{CellIdx, GridKind, LaneIdx};
use number_loom::gui::FamilyIdx;
use number_loom::puzzle::{
    BACKGROUND, Clue, Color, Document, DynSolution, PartialSolution, Puzzle,
};
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
/// Where the web app lives; a `#WOVEN-...-` after it opens that puzzle.
const APP_URL: &str = "https://paulstansifer.github.io/number-loom/";
/// The environment variable with the Reddit username (without the `u/`) of whoever runs the bot.
const OPERATOR_VAR: &str = "REDDIT_OPERATOR";

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
    Stuck(),
    /// These lanes can't be completed as they stand. (Mistakes are only caught where the answer
    /// is known, so this is possible when it isn't all known.)
    Broken(Vec<LaneIdx>),
}

#[derive(Clone, Debug)]
pub struct GuidanceReport {
    pub solved: Solved,
    /// What's known of the answer: all of it, unless `solved` says otherwise.
    pub answer: PartialSolution,
    /// Number of cells the user filled in:
    pub filled_cells: usize,
    /// The cells the person has got wrong.
    pub errors: Vec<CellIdx>,
    /// Where to go from here, once the errors are erased.
    pub next: Next,
    /// A link that opens the puzzle in the web app, to solve from the beginning.
    pub start_url: Option<String>,
    /// Like `start_url`, but with the person's progress (with the errors erased) filled in.
    pub progress_url: Option<String>,
}

/// Emits a Markdown message about the puzzle. `repairs` is how many clues couldn't be read, and
/// were worked out from the grid instead (see `repair`).
/// Unlike everything else in this directory; this is human-written;
/// LLMs should add TODOs if necessary, but not change any text.
pub fn guidance_to_message<C: Clue, K: GridKind>(
    puz: &Puzzle<C, K>,
    g: &GuidanceReport,
    repairs: usize,
) -> String {
    let mut res = String::new();

    let like_what = match g.solved {
        Solved::Ambiguous => " with multiple solutions.",
        Solved::LineLogic => ", solvable with line logic.",
        Solved::Search => ", only solvable with trial-and-error.",
        Solved::OutOfTime => ", which I was unable to solve!",
    };
    // Fortunately, numbers <80 that start with a vowel sound aren't a multiple of five, so we're unlikely to
    // need to say "an".
    res.push_str(&format!(
        "This looks like a {} puzzle{like_what} ",
        puz.geometry.dims_label()
    ));
    if repairs > 0 {
        let pl = if repairs == 1 { "" } else { "s" };
        res.push_str(&format!(
            "(OCR failed for {repairs} clue{pl}, but they could be extracted from the grid.) "
        ));
    }
    if !g.errors.is_empty() {
        let pl = if g.errors.len() == 1 { "" } else { "s" };
        res.push_str("\n"); // Want to start this on its own line.
        res.push_str(&format!(
            "If I read the grid correctly, there are {} mistake{pl} to remove: >!",
            g.errors.len(),
        ));

        let mut first = true;
        for error in g.errors.iter().take(5) {
            if !first {
                res.push_str(", ");
            }
            first = false;

            let loc = K::coord_label(puz.geometry.coord(*error));
            res.push_str(&format!("{loc}"));
        }

        if g.errors.len() > 5 {
            res.push_str(&format!(", and {} more", g.errors.len() - 5));
        }
        res.push_str("!<\n");
    } else if matches!(g.next, Next::Done) {
        match g.solved {
            Solved::LineLogic => {
                res.push_str("You solved it correctly!\n");
            }
            Solved::Search => {
                res.push_str("You solved it correctly!!\n");
            }
            Solved::Ambiguous => {
                res.push_str("You found one of its solutions!\n");
            }
            Solved::OutOfTime => {
                res.push_str("Nonetheless, you solved it!\n");
            }
        }
    } else if g.filled_cells > 0 {
        if matches!(g.solved, Solved::LineLogic | Solved::Search) {
            res.push_str(&format!(
                "You've solved {:.1}% of the puzzle correctly.\n",
                g.filled_cells as f32 / g.answer.len() as f32 * 100.0
            ));
        } else {
            res.push_str(&format!(
                "You've filled in {:.1}% of the puzzle.\n",
                g.filled_cells as f32 / g.answer.len() as f32 * 100.0
            ));
        }
    }
    res.push_str("\n\n");

    if let Some(s_url) = &g.start_url {
        res.push_str(&format!("[Start from scratch]({s_url})"));
        if let Some(c_url) = &g.progress_url {
            res.push_str(&format!(" or [continue from here]({c_url})"));
        }
        res.push_str(&format!(
            " in [Number Loom](https://paul-stansifer.itch.io/number-loom).\n\n"
        ))
    }

    match &g.next {
        Next::Done => {} // handled above
        Next::Lines(lhes) | Next::LinesAfterPartial(lhes) => {
            if lhes.len() == 1 {
                res.push_str("There is one lane ");
            } else {
                res.push_str(&format!("There are {} lanes ", lhes.len()));
            }
            res.push_str("you can progress with line logic");
            if matches!(g.next, Next::LinesAfterPartial(_)) {
                res.push_str(" (but only if you cross-reference color information)");
            }
            if lhes.len() > 5 {
                res.push_str(" (here's the first five)");
            }
            res.push_str(":\n\n");
            for lh in lhes.iter().take(5) {
                let (fam, lane) = puz.lane_map().split_family(lh.lane);
                // This would be badly wrong for triddlers!
                let fam_str = if fam == FamilyIdx(0) { "R" } else { "C" };
                let idx = lane + 1;
                let pl = if lh.resolves.len() == 1 { "" } else { "s" };
                res.push_str(&format!(
                    " * >!{fam_str}{idx}, which can resolve {} cell{pl}!<\n",
                    lh.resolves.len()
                ));
            }
        }
        Next::Contradictions(cons) => {
            if cons.len() == 1 {
                res.push_str("There is at least one cell ");
            } else {
                res.push_str(&format!("There are at least {} cells ", cons.len()));
            }
            res.push_str("that can be guessed and disproven:\n\n");
            for (cell, color) in cons {
                let loc = K::coord_label(puz.geometry.coord(*cell));
                let color_str = if *color == BACKGROUND {
                    "background"
                } else {
                    "colored-in" // currently, we don't handle multicolor puzzles
                };
                res.push_str(&format!(" * >!{loc} can't be {color_str}!<\n"));
            }
        }
        Next::Stuck() => {
            if g.solved == Solved::Search {
                res.push_str("This puzzle probably requires nested guesses at this point. It's quite hard!\n");
            } else {
                res.push_str("There's also no obvious way to make progress.\n");
            }
        }
        Next::Broken(bad_lanes) => {
            res.push_str("The current grid is already contradictory in the following lanes:\n\n");
            for bad_lane in bad_lanes.iter().take(5) {
                let (fam, cell) = puz.lane_map().split_family(*bad_lane);
                // This would be badly wrong for triddlers!
                let fam_str = if fam == FamilyIdx(0) { "R" } else { "C" };
                let idx = cell + 1;
                res.push_str(&format!(" * {fam_str}{idx}\n"));
            }
        }
    }

    // (Without an operator to name, there's no sign-off.)
    if let Ok(operator) = std::env::var(OPERATOR_VAR) {
        res.push_str(&format!("\n\n^(This is a bot, operated by [u/{operator}](https://www.reddit.com/user/{operator}/))"));
    }

    res
}

/// Guidance for someone who's gotten as far as `grid` in `puzzle`. An error if the clues have no
/// solution.
pub fn guidance<C: Clue, K: GridKind>(
    puzzle: &Puzzle<C, K>,
    grid: &PartialSolution,
) -> anyhow::Result<GuidanceReport> {
    let (solution, solved) = find_answer(puzzle)?;
    let answer = solution.to_partial();
    let all_known = matches!(solved, Solved::LineLogic | Solved::Search);
    let start_url = if all_known {
        Some(share_url(solution.clone(), None)?)
    } else {
        None
    };

    let mut grid = grid.clone();
    let filled_cells = grid.iter().filter(|c| c.is_known()).count();
    let mut errors = vec![];
    for (cell, mine) in grid.iter_mut_enumerated() {
        // (A mistake is ruling out the answer.)
        let mut both = *mine;
        if both.learn_intersect(answer[cell]).is_err() {
            errors.push(cell);
            *mine = Cell::new(&puzzle.palette);
        }
    }

    let progress_url = if all_known
        && !grid.iter().all(Cell::is_known)
        && has_progress(&grid.raw, puzzle.palette.keys().copied())
    {
        Some(share_url(solution, Some(grid.clone()))?)
    } else {
        None
    };

    let next = if grid.iter().all(Cell::is_known) {
        // Where the answer isn't all known, agreeing with it doesn't mean agreeing with the clues.
        match scrub_lanes(puzzle, &grid) {
            Ok(_) => Next::Done,
            Err(broken) => Next::Broken(broken),
        }
    } else {
        next_step(puzzle, grid)?
    };
    Ok(GuidanceReport {
        solved,
        answer,
        filled_cells,
        errors,
        next,
        start_url,
        progress_url,
    })
}

/// A link that opens `solution`'s puzzle in the web app, as far along as `in_progress`.
fn share_url(
    solution: DynSolution,
    in_progress: Option<PartialSolution>,
) -> anyhow::Result<String> {
    let mut doc = Document::new(
        None,
        Some(solution),
        in_progress,
        String::new(),
        None,
        None,
        None,
        None,
        None,
    );
    let woven = to_woven(&mut doc)?;
    // (It comes broken into lines.)
    let woven: String = woven.chars().filter(|c| !c.is_whitespace()).collect();
    Ok(format!("{APP_URL}#{woven}"))
}

/// Line logic, and then, if that doesn't finish the job, a search (for a while). Cells that
/// aren't known are `UNSOLVED`.
fn find_answer<C: Clue, K: GridKind>(
    puzzle: &Puzzle<C, K>,
) -> anyhow::Result<(DynSolution, Solved)> {
    let report = grid_solve::solve(puzzle, &mut None, &SolveOptions::default())?;
    let line_logic = report.solution;
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
        Ok(report) if report.cells_left == 0 => Ok((report.solution, Solved::Search)),
        // (When it finds a second solution, it reports what's known without guessing.)
        Ok(report) => Ok((report.solution, Solved::Ambiguous)),
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
    stalled.run(&mut ctx)?;

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
        if supposing.learn(&mut ctx, cell, true, color).is_err() || supposing.run(&mut ctx).is_err()
        {
            contradictions.push((cell, color));
        }
    }
    Ok(if contradictions.is_empty() {
        Next::Stuck()
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
        assert!(report.progress_url.is_none());
    }

    #[test]
    fn the_link_opens_the_same_puzzle() {
        let p = puzzle(&["#####", "#....", "#.###", "#...#", "#####"]);
        let report = guidance(
            &p,
            &progress(&p, &[".....", ".....", ".....", ".....", "....."]),
        )
        .unwrap();
        let mut doc = open_link(&report.start_url.unwrap());
        assert_eq!(doc.solution().unwrap().to_partial(), report.answer);
        assert!(doc.in_progress().is_none());
        // (There's nothing to show yet.)
        assert!(report.progress_url.is_none());
    }

    #[test]
    fn the_progress_link_has_the_mistakes_erased() {
        let p = puzzle(&["#####", "#....", "#.###", "#...#", "#####"]);
        let report = guidance(
            &p,
            &progress(&p, &["#####", "x#...", ".....", ".....", "....."]),
        )
        .unwrap();
        let mut doc = open_link(&report.progress_url.unwrap());
        assert_eq!(doc.solution().unwrap().to_partial(), report.answer);
        // (Undecided cells come back as able to be anything, not just anything in the palette.)
        let decided = |grid: &PartialSolution| -> Vec<Option<Color>> {
            grid.iter()
                .map(|c| c.is_known().then(|| c.unwrap_color()))
                .collect()
        };
        assert_eq!(
            decided(doc.in_progress().unwrap()),
            decided(&progress(
                &p,
                &["#####", ".....", ".....", ".....", "....."]
            ))
        );
    }

    /// The document a link from `guidance` opens.
    fn open_link(url: &str) -> Document {
        let woven = url.strip_prefix(&format!("{APP_URL}#")).unwrap();
        number_loom::formats::woven::from_woven(woven, String::new()).unwrap()
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
        assert!(report.start_url.is_none());
        assert!(report.progress_url.is_none());
        assert!(
            matches!(report.next, Next::Stuck { .. }),
            "{:?}",
            report.next
        );
    }

    #[test]
    fn filled_in_but_wrong_in_an_ambiguous_puzzle() {
        // Nothing's known of the answer, so nothing counts as a mistake, but the clues disagree.
        let p = puzzle(&["#.", ".#"]);
        let report = guidance(&p, &progress(&p, &["##", "##"])).unwrap();
        assert_eq!(report.solved, Solved::Ambiguous);
        assert!(report.errors.is_empty());
        assert!(matches!(report.next, Next::Broken(_)), "{:?}", report.next);
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
