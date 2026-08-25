use std::{collections::HashMap, ops::Mul};

use anyhow::{Context, bail};
use itertools::Itertools;
use priority_queue::PriorityQueue;

use crate::{
    bt_solve::BtReport::{MultipleSolutions, UniqueSolution},
    geometry::GridKind,
    grid_solve::{LineCache, Report, SolveContext, SolveOptions, SolveState},
    line_solve::ModeMap,
    puzzle::{Clue, Color, PartialSolution, Puzzle},
};

/// A coordinate into the tree of hypotheticals
#[derive(PartialEq, Eq, Hash, Debug, Clone)]
struct HypoCoord {
    guesses: Vec<(usize, Color)>,
}

#[derive(Clone)]
struct BtSolveState<'p, C: Clue> {
    knowledge: SolveState<'p, C>,
    // TODO: add geometry (needs K: GridKind)
    guesses_explored: Vec<(usize, Color)>,
}

impl<'p, C: Clue> BtSolveState<'p, C> {
    fn score_at(&self, coord: &HypoCoord) -> Score {
        let distance_remaining = self.knowledge.cells_left as f32;
        let depth = 5.0 * 2.0_f32.powi(coord.guesses.len() as i32);
        let exhaustion = self.guesses_explored.len() as f32 * 3.0;
        Score(distance_remaining + depth + exhaustion)
    }
}

trait GuessPicker {
    /// Score guesses against each other. Note that this is totally different than *node* scores!
    fn rate<'p, C: Clue, K: GridKind>(state: &BtSolveState<'p, C>, guess: (usize, Color)) -> Score;

    /// Pick the lowest-scoring
    fn pick<'p, C: Clue, K: GridKind>(state: &BtSolveState<'p, C>) -> (usize, Color) {
        let idxed_cells = state.knowledge.grid.iter().enumerate();
        let possibilities =
            idxed_cells.flat_map(|(idx, cell)| cell.can_be_iter().map(move |color| (idx, color)));
        let mut ranked = possibilities
            .sorted_by_cached_key(|(idx, color)| Self::rate::<C, K>(state, (*idx, *color)));
        ranked
            .next()
            .expect("A node with no unsolved cells shouldn't be examined!")
    }
}

struct First;

impl GuessPicker for First {
    fn rate<'p, C: Clue, K: GridKind>(_: &BtSolveState<'p, C>, _: (usize, Color)) -> Score {
        Score(0.0)
    }
}

struct Edge;

impl GuessPicker for Edge {
    fn rate<'p, C: Clue, K: GridKind>(
        state: &BtSolveState<'p, C>,
        (idx, col): (usize, Color),
    ) -> Score {
        todo!()
    }
}

/// Lower is better! This is used both to score nodes (`score_at`) and to score possible guesses inside nodes (`rate`)!
#[derive(PartialEq, PartialOrd)]
struct Score(f32);

impl Eq for Score {}

impl Ord for Score {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.partial_cmp(other).unwrap()
    }
}

// Presumably we need to do some score-boosting here
fn learn_from_contradiction<'p, 'x, C: Clue, K: GridKind>(
    coord: &HypoCoord,
    cell_idx: usize,
    color: Color,
    q: &mut PriorityQueue<HypoCoord, Score>,
    possibilities: &mut HashMap<HypoCoord, BtSolveState<'p, C>>,
    ctx: &mut SolveContext<'p, 'x, C, K>,
) -> anyhow::Result<()> {
    let state = possibilities.get_mut(&coord).unwrap();
    state.knowledge.grid[cell_idx].learn_that_not(color)?;

    let higher_result = state.knowledge.run(ctx);
    q.change_priority(coord, state.score_at(coord)); // Did all that learning make this node look better?

    // TODO: the new knowledge at this level *ought* to be applied to all descendant nodes.
    // ...or those nodes should be cleared out.
    // ...or we should apply it lazily (but maybe give them a score boost since they might be advanceable?)

    if let Err(e) = higher_result {
        let mut higher_coord = coord.clone();
        if let Some((prev_cell_idx, prev_color)) = higher_coord.guesses.pop() {
            learn_from_contradiction(coord, prev_cell_idx, prev_color, q, possibilities, ctx)?;
        } else {
            // oops; end of the line!
            return Err(e).context("after counterfactual deduction");
        }
    }

    Ok(())
}

pub enum BtReport {
    MultipleSolutions(), // TODO: provide *some* information
    UniqueSolution(Report),
}

pub fn backtrack_solve<C: Clue, K: GridKind>(
    puzzle: &Puzzle<C, K>,
    options: &SolveOptions,
    grid: &mut PartialSolution,
) -> anyhow::Result<BtReport> {
    let mut line_cache: Option<LineCache<C>> = Some(LineCache::new());
    let mut ctx = SolveContext::new(puzzle, &mut line_cache, options);
    let mut init_linear_state = SolveState::new(&mut ctx, std::mem::take(grid));

    init_linear_state.run(&mut ctx)?; // `?` because contradictions here are "real"

    if init_linear_state.cells_left == 0 {
        return Ok(UniqueSolution(init_linear_state.report(puzzle))); // No backtracking required!
    }

    let mut solutions_found = 0;

    let root_coord = HypoCoord { guesses: vec![] };
    let mut possibilities: HashMap<HypoCoord, BtSolveState<C>> = HashMap::default();
    let mut q: PriorityQueue<HypoCoord, Score> = PriorityQueue::new();

    q.push(root_coord.clone(), Score(0.0));
    possibilities.insert(
        root_coord,
        BtSolveState {
            knowledge: init_linear_state,
            guesses_explored: vec![],
        },
    );

    while let Some((coord, _score)) = q.pop() {
        let mut new_state = possibilities[&coord].clone();
        let (cell_idx, color) = First::pick::<C, K>(&new_state);

        new_state.knowledge.grid[cell_idx].is_known_to_be(color); // make the assumption!
        let result = new_state.knowledge.run(&mut ctx);

        match result {
            Err(_) => {
                // Maybe we learned something for real!
                learn_from_contradiction(
                    &coord,
                    cell_idx,
                    color,
                    &mut q,
                    &mut possibilities,
                    &mut ctx,
                )? // (... maybe we learned too much!)
            }
            Ok(_) => {
                if new_state.knowledge.cells_left == 0 {
                    if coord.guesses.is_empty() {
                        // TODO: the solve counts will be misleadingly low!
                        return Ok(UniqueSolution(new_state.knowledge.report(puzzle)));
                    }

                    solutions_found += 1;
                    if solutions_found > 1 {
                        return Ok(MultipleSolutions());
                    }

                    continue; // don't need to search `new_coord` any more!
                }
            }
        }

        let mut new_coord = coord.clone();
        new_coord.guesses.push((cell_idx, color));

        q.push(new_coord.clone(), new_state.score_at(&new_coord));
        possibilities.insert(new_coord, new_state);
    }
    unreachable!(
        "The root state shouldn't be popped-and-not-pushed without finding a unique solution"
    );
}
