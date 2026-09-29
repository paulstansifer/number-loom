use std::collections::{HashMap, HashSet};

use typed_index_collections::TiVec;

use anyhow::bail;
use rand::{SeedableRng, rngs::StdRng};

use crate::{
    geometry::{CellIdx, GridKind, LaneIdx, LanePos},
    puzzle::{Clue, Color, PartialSolution, Puzzle},
    solve::{
        conprop_picker::Picker,
        grid_solve::{
            LineCache, Report, SolveContext, SolveOptions, SolveState, TrailReason, TrailStep,
            gather_into,
        },
        line_solve::{Cell, exhaust_line},
    },
};

/// Why `propagate` found the current branch impossible.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Conflict {
    /// Line logic found no arrangement of this lane that fits the grid as it stands.
    Lane(LaneIdx),
    /// Every literal of this nogood (an index into `nogoods`) holds.
    Nogood(usize),
}

#[derive(Clone)]
struct Nogood {
    not_all_true: HashSet<(CellIdx, Color)>,
    current_false_count: usize,
    active: bool,
}

impl Nogood {
    /// An error if the nogood is already contradicted, `Ok(Some(cell, color))` if `cell` can't be `color`
    fn deduction<C: Clue>(
        &self,
        ll_state: &SolveState<C>,
    ) -> anyhow::Result<Option<(CellIdx, Color)>> {
        if self.active {
            // assumptions are already applied
            return Ok(None);
        }
        let remaining = self.not_all_true.len() - self.current_false_count;
        if remaining > 1 {
            return Ok(None);
        }
        if remaining == 0 {
            debug_assert!(
                self.not_all_true
                    .iter()
                    .all(|&(cell_idx, color)| ll_state.grid[cell_idx].is_known_to_be(color)),
                "Claude was right to be worried about this"
            );
            bail!("Nogood contradicted")
        }

        for &(cell_idx, color) in &self.not_all_true {
            let cell = &ll_state.grid[cell_idx];
            if !cell.is_known_to_be(color) {
                return Ok(Some((cell_idx, color)));
            }
            if !cell.can_be(color) {
                return Ok(None); // This nogood can't be relevant
            }
        }
        bail!("Nogood contradicted, and `current_false_count` was stale");
    }

    fn inform(&mut self, cell_idx: CellIdx, color: Color, forwards: bool) {
        debug_assert_eq!(
            // `nogoods_by_cell` should mean that `cell_idx` is always relevant
            self.not_all_true
                .iter()
                .map(|t| t.0)
                .filter(|idx| *idx == cell_idx)
                .count(),
            1
        );

        if self.not_all_true.contains(&(cell_idx, color)) {
            if forwards {
                self.current_false_count += 1;
                debug_assert!(self.current_false_count <= self.not_all_true.len());
            } else {
                // This is being unwound, along with its consequences (or it already was)
                self.active = false;
                self.current_false_count = self.current_false_count.strict_sub(1);
            }
        }
    }
}

// TODO: there are a bunch of indices; maybe type them, too.
#[derive(Clone)]
pub struct ConpropState<'p, C: Clue> {
    nogoods: Vec<Nogood>,
    // Outer is indexable by `cell_idx`, inner contains indices to `nogoods`
    nogoods_by_cell: TiVec<CellIdx, Vec<usize>>,
    pub vsids: HashMap<(CellIdx, Color), f32>,
    vsids_decay: u8,

    trail: Vec<TrailStep>,

    nogoods_updated_to: usize, // index into trail: where are the nogoods current up to?
    guesses_in_trail: Vec<(usize, Color)>, // (index into trail, guessed color)
    pickers: Vec<Picker>,
    rng: StdRng, //Only used by the picker.
    pub ll_state: SolveState<'p, C>,

    guesses_made: usize,
    // TODO: feed this to the picker, so it only picks things that contradict this
    solution_found: Option<PartialSolution>, // never actually "Partial", of course.
    /// Everything we know at the root (without assumptions) when a first solution is found.
    /// (The search for a second solution requires assuming the first one is wrong).
    root_knowledge: Option<(PartialSolution, usize)>,
}

impl<'p, C: Clue> ConpropState<'p, C> {
    pub fn new(ll_state: SolveState<'p, C>) -> Self {
        ConpropState {
            nogoods: vec![],
            nogoods_by_cell: vec![vec![]; ll_state.grid.len()].into(),
            vsids: HashMap::default(),
            vsids_decay: 0,
            trail: vec![],
            nogoods_updated_to: 0,
            guesses_in_trail: vec![],
            ll_state,
            pickers: vec![],
            rng: StdRng::seed_from_u64(0),
            guesses_made: 0,
            solution_found: None,
            root_knowledge: None,
        }
    }

    // Typically, you'll call `propagate{,_and_learn}` after this.
    // `Err(_)` if the guess is inherently wrong, `Ok(false)` if the guess was already true.
    // Only makes changes if it returns `Ok(true)`
    // TODO: maybe just return a bool?
    fn make_guess<'x, K: GridKind>(
        &mut self,
        (cell_idx, color): (CellIdx, Color),
        linear_ctx: &mut SolveContext<'p, 'x, C, K>,
    ) -> anyhow::Result<bool> {
        let old_cell = self.ll_state.grid[cell_idx];
        let res = self
            .ll_state
            .learn(linear_ctx, cell_idx, /*is*/ true, color)?;
        if res {
            self.guesses_made += 1;
            self.guesses_in_trail.push((self.trail.len(), color));
            // TODO: fold the `trail` update in `.learn_new` ... if this wins out over `bt_solve`
            self.trail.push(TrailStep {
                cell_idx,
                old_value: old_cell,
                reason: TrailReason::Guess,
            });
        }

        Ok(res)
    }

    /// Rewind to just *before* `guess_idx` (including its consequences)...
    /// This pops stuff off `trail` and undoes progress towards `nogoods`
    fn backjump<'x, K: GridKind>(
        &mut self,
        guess_idx: usize,
        linear_ctx: &mut SolveContext<'p, 'x, C, K>,
    ) {
        if self.guesses_in_trail.is_empty() {
            assert_eq!(guess_idx, 0); // We do `backjump(0)` without checking
            return;
        }
        debug_assert_eq!(self.nogoods_updated_to, self.trail.len());

        let (trail_idx, _guess) = self.guesses_in_trail[guess_idx];
        self.update_nogood_counters(trail_idx);
        self.ll_state.unwind(linear_ctx, &self.trail[trail_idx..]);

        self.trail.truncate(trail_idx);
        self.guesses_in_trail.truncate(guess_idx);
        self.pickers.truncate(guess_idx + 1);
    }

    /// Start watching a nogood.
    fn add_nogood(&mut self, nogood: Nogood) {
        let nogood_idx = self.nogoods.len();
        for &(cell_idx, color) in &nogood.not_all_true {
            self.nogoods_by_cell[cell_idx].push(nogood_idx);

            *self.vsids.entry((cell_idx, color)).or_insert(0.0) += 1.0;
        }
        self.nogoods.push(nogood);

        if self.vsids_decay >= 16 {
            for (_, score) in &mut self.vsids {
                *score *= 0.95;
            }
            self.vsids_decay = 0;
        } else {
            self.vsids_decay += 1;
        }
    }

    /// Make a simple nogood from the trail. This is used to *rule out* a valid solution
    /// (whatever complete solution these implied) in hopes of finding a different one.
    fn solution_nogood(&self) -> Nogood {
        let not_all_true: HashSet<(CellIdx, Color)> = self
            .guesses_in_trail
            .iter()
            .map(|&(trail_idx, color)| (self.trail[trail_idx].cell_idx, color))
            .collect();

        Nogood {
            current_false_count: not_all_true.len(), // every one of them holds right now
            not_all_true,
            active: false,
        }
    }

    /// Adjust any `Nogood`s that are out-of-date.
    fn update_nogood_counters(&mut self, new_idx: usize) {
        if new_idx == self.nogoods_updated_to {
            return;
        }

        let forwards = new_idx > self.nogoods_updated_to;
        let range = if forwards {
            self.nogoods_updated_to..new_idx
        } else {
            new_idx..self.nogoods_updated_to
        };

        let mut cells_seen = HashSet::<CellIdx>::new();
        for &TrailStep {
            cell_idx,
            old_value,
            ..
        } in &self.trail[range]
        {
            if !cells_seen.insert(cell_idx) {
                continue; // don't double-count! All we're doing here is seeing whether the cell became known
            }
            if old_value.is_known() {
                panic!("Made a redundant guess: {cell_idx:?} {old_value:?}"); // `continue` would be safe.
            }

            // High end knows it:
            if let Some(known_color) = self.ll_state.grid[cell_idx].known_or() {
                for &nogood_idx in &self.nogoods_by_cell[cell_idx] {
                    self.nogoods[nogood_idx].inform(cell_idx, known_color, forwards);
                }
            } else if !forwards {
                for &nogood_idx in &self.nogoods_by_cell[cell_idx] {
                    // HACK: otherwise single-entry nogoods would never be deactivated.
                    // But why do single-entry nogoods *need* to be destroyed?
                    self.nogoods[nogood_idx].active = false;
                }
            }
        }

        self.nogoods_updated_to = new_idx;
    }

    /// Applies linear logic and nogoods until everything possible is deduced. `Ok(true)` if a solution is found.
    ///
    /// On a `Conflict`, the grid is left as the conflict found it, so explain it before unwinding.
    fn propagate<'x, K: GridKind>(
        &mut self,
        linear_ctx: &mut SolveContext<'p, 'x, C, K>,
    ) -> Result<bool, Conflict> {
        let mut any_nogoods_fired = true;
        while any_nogoods_fired {
            any_nogoods_fired = false;

            let linear_res = self
                .ll_state
                .run_and_check_recording(linear_ctx, &mut self.trail);
            self.update_nogood_counters(self.trail.len());

            // Had to update the counters first.
            linear_res.map_err(|contradiction| Conflict::Lane(contradiction.lane))?;

            for nogood_idx in 0..self.nogoods.len() {
                self.update_nogood_counters(self.trail.len()); // Get it right before `deduction`.

                let nogood = &mut self.nogoods[nogood_idx];

                let deduction = nogood
                    .deduction(&self.ll_state)
                    .map_err(|_| Conflict::Nogood(nogood_idx))?;
                if let Some((cell_idx, is_not_color)) = deduction {
                    any_nogoods_fired = true;
                    nogood.active = true;

                    let before = self.ll_state.grid[cell_idx];
                    // An error here means the implication of the nogood contradicts what we
                    // already know.
                    let learned = self
                        .ll_state
                        .learn(linear_ctx, cell_idx, /*is=*/ false, is_not_color)
                        .map_err(|_| Conflict::Nogood(nogood_idx))?;
                    if learned {
                        // Only when it actually moved: a no-op entry is a cell the trail claims
                        // changed when it didn't, and `update_nogood_counters` believes the trail.
                        self.trail.push(TrailStep {
                            cell_idx,
                            old_value: before,
                            reason: TrailReason::Nogood(nogood_idx),
                        });
                    }
                }
            }
        }

        Ok(self.ll_state.cells_left == 0)
    }

    fn propagate_and_learn<'x, K: GridKind>(
        &mut self,
        puzzle: &Puzzle<C, K>,
        linear_ctx: &mut SolveContext<'p, 'x, C, K>,
    ) -> Option<Report> {
        let state = self;
        let mut run_res = state.propagate(linear_ctx);
        while let Err(conflict) = run_res {
            if state.guesses_in_trail.is_empty() {
                // We would create an empty nogood (which isn't supported),
                // indicating an unsolvable puzzle.
                return Some(state.no_guesses_left(puzzle));
            }
            // We get a nogood from the guesses...
            let (guess_nogood, mut backjump_dest) = state.guess_is_nogood(linear_ctx);
            state.add_nogood(guess_nogood);

            let lane_nogood = match conflict {
                // ...and maybe another one from the line where the conflict happened:
                Conflict::Lane(lane_idx) => state.line_is_nogood(lane_idx, linear_ctx),
                Conflict::Nogood(_) => None,
            };

            if let Some((lane_nogood, asserting_at)) = lane_nogood {
                state.add_nogood(lane_nogood);
                if let Some(asserting_at) = asserting_at {
                    // Both backjumps are valid; take the more aggressive one:
                    backjump_dest = backjump_dest.min(asserting_at);
                }
            }
            state.backjump(backjump_dest, linear_ctx);

            run_res = state.propagate(linear_ctx)
        }
        if run_res.unwrap() {
            // We found a valid solution!
            if let Some(existing_solution) = &state.solution_found {
                assert!(
                    *existing_solution != state.ll_state.grid,
                    "TODO: I thought we couldn't reach the same solution multiple times"
                );

                // Multiple valid solutions: report our snapshot of the cells we proved.
                let (grid, cells_left) = state
                    .root_knowledge
                    .as_ref()
                    .expect("a first solution always leaves its root behind");
                return Some(Report::from_grid(
                    puzzle,
                    grid,
                    *cells_left,
                    state.ll_state.solve_counts,
                ));
            }
            // Record first solution:
            state.solution_found = Some(state.ll_state.grid.clone());

            if linear_ctx.options.stop_at_first_solution {
                // `cells_left` is a bit of a lie, since we don't know the solution is unique
                return Some(Report::from_grid(
                    puzzle,
                    &state.ll_state.grid,
                    /*cells_left=*/ 0,
                    state.ll_state.solve_counts,
                ));
            }

            if state.guesses_in_trail.is_empty() {
                // Nice, no guesses outstanding. We know it's unique.
                return Some(Report::from_grid(
                    puzzle,
                    &state.ll_state.grid,
                    /*cells_left=*/ 0,
                    state.ll_state.solve_counts,
                ));
            }

            // Move the goalposts: now try to find a second solution.
            let first_solution_is_nogood = state.solution_nogood();
            state.add_nogood(first_solution_is_nogood);

            // Now go back and try again!
            state.backjump(0, linear_ctx);
            // Record what we know without any assumptions (and before using the fake nogood)
            state.root_knowledge = Some((state.ll_state.grid.clone(), state.ll_state.cells_left));

            return state.propagate_and_learn(puzzle, linear_ctx);
        } else {
            return None; // No solution, no error.
        }
    }

    /// Turn a contradiction derived at `lane_idx` into a nogood. (Or `None` if partial knowledge
    /// in a color puzzle makes it hard.)
    /// Also returns the backjump destination implied by the nogood (Or `None`)
    ///
    /// This should be called with the contradiction still in the grid.
    fn line_is_nogood<'x, K: GridKind>(
        &self,
        lane_idx: LaneIdx,
        linear_ctx: &SolveContext<'p, 'x, C, K>,
    ) -> Option<(Nogood, Option<usize>)> {
        let lane_cells = &linear_ctx.lane_map().lanes[lane_idx].cells;
        let clues = self.ll_state.lanes[lane_idx].clues;

        let contradicts = |lane: &[Cell]| exhaust_line(clues, &mut lane.to_vec()).is_err();

        let mut now = vec![];
        // TODO: we `pub`ed `gather_into` and `.clue` just for this function; can we be less ad-hoc?
        gather_into(
            linear_ctx.lane_map(),
            lane_idx,
            &self.ll_state.grid,
            &mut now,
        );
        if !contradicts(&now) {
            debug_assert!(false, "{lane_idx:?} was blamed, but it's satisfiable");
            return None;
        }

        let mut probe = now.clone();

        // We want to know the "root" state (unconditional knowledge); scan the trail to find what
        // has changed since then.
        // We also want to know what level (how many guesses) a literal is settled at.

        // TODO: why isn't `first_guess_at` always 0?
        let first_guess_at = self.guesses_in_trail.first()?.0;
        let lane_pos_of: HashMap<CellIdx, usize> = lane_cells
            .iter()
            .enumerate()
            .map(|(pos, &cell_idx)| (cell_idx, pos))
            .collect();
        let mut root = now.clone();
        let mut settled_at: Vec<usize> = vec![0; now.len()];
        for (
            trail_idx,
            &TrailStep {
                cell_idx,
                old_value,
                ..
            },
        ) in self.trail.iter().enumerate().skip(first_guess_at)
        {
            if let Some(&pos) = lane_pos_of.get(&cell_idx) {
                if settled_at[pos] == 0 {
                    // First place it got set: the root value was stored
                    root[pos] = old_value;
                }
                settled_at[pos] = trail_idx;

                // We can't express partial knowledge in nogoods, so erase partial knowledge...
                if !probe[pos].is_known() {
                    probe[pos] = root[pos];
                }
            }
        }

        // ...but if the partial knowledge was important, then we gotta give up:
        if !contradicts(&probe) {
            return None;
        }

        // Try putting each cell back to its root value...
        for pos in 0..probe.len() {
            if probe[pos] == root[pos] {
                continue;
            }
            let prob_orig = probe[pos];
            probe[pos] = root[pos];
            if !contradicts(&probe) {
                // ...oops; that was needed to get the contradiction!
                probe[pos] = prob_orig;
            }
        }

        let kept: Vec<usize> = (0..probe.len())
            .filter(|&pos| probe[pos] != root[pos])
            .collect();
        // The root lane was consistent when we left it, so something must have survived.
        debug_assert!(!kept.is_empty(), "{lane_idx:?} contradicts at the root");

        // A literal's level is how many guesses came at or before the entry that settled it.
        let mut levels: Vec<usize> = kept
            .iter()
            .map(|&pos| {
                self.guesses_in_trail
                    .partition_point(|&(guess_at, _)| guess_at <= settled_at[pos])
            })
            .collect();
        levels.sort_unstable();
        let asserting_at = match levels.as_slice() {
            [.., second_deepest, deepest] if second_deepest < deepest => Some(*second_deepest),
            [_] => Some(0),
            _ => None,
        };

        let not_all_true: HashSet<(CellIdx, Color)> = kept
            .iter()
            .map(|&pos| (lane_cells[LanePos::from(pos)], probe[pos].unwrap_color()))
            .collect();
        let nogood = Nogood {
            current_false_count: not_all_true.len(),
            not_all_true,
            active: false,
        };
        Some((nogood, asserting_at))
    }

    /// Minimizes the tail of the `Nogood`, and returns an index (to `guesses_in_trail`) to backjump past,
    /// or `None` if all the guesses should be cleared.
    fn guess_is_nogood<'x, K: GridKind>(
        &self,
        linear_ctx: &mut SolveContext<'p, 'x, C, K>,
    ) -> (Nogood, usize) {
        // TODO: the clone is expensive: we might want to try reusing `self` (and restoring it after)
        let mut exp_state = self.clone();
        exp_state.backjump(0, linear_ctx); // Back to the root, to try a different order
        exp_state.pickers.clear(); // We'll be replaying guesses; don't need pickers.

        let mut res = Nogood {
            not_all_true: HashSet::new(),
            current_false_count: 0,
            active: false,
        };

        let Some((last_guess, pfx_guesses)) = self.guesses_in_trail.split_last() else {
            return (res, 0); // No guesses, so the puzzle is contradictory.
        };

        for (after_guess_idx, &(guess_idx_in_trail, color)) in std::iter::once(last_guess)
            .chain(pfx_guesses.iter())
            .enumerate()
        {
            let cell_idx = self.trail[guess_idx_in_trail].cell_idx;
            if exp_state.ll_state.grid[cell_idx].is_known_to_be(color) {
                continue; // no need to guess what we already know
            }
            // if it's known to be another color, we will get a useful contradiction in `.make_guess`

            res.not_all_true.insert((cell_idx, color));
            res.current_false_count += 1;

            let guess_ok = exp_state.make_guess((cell_idx, color), linear_ctx).is_ok()
                && exp_state.propagate(linear_ctx).is_ok();

            if !guess_ok {
                return (res, after_guess_idx);
            }
        }
        panic!("Should've re-found that contradiction");
    }

    /// We have tried everything.
    /// (The trail isn't necessarily empty when this is called: a root-level nogood deduction is
    /// on it too, and it stays there when the last guess comes off.)
    fn no_guesses_left<K: GridKind>(&self, puzzle: &Puzzle<C, K>) -> Report {
        if let Some(solution) = &self.solution_found {
            // Nothing is left to try, so the grid we completed is the *only* one that fits.
            Report::from_grid(
                puzzle,
                solution,
                /*cells_left=*/ 0,
                self.ll_state.solve_counts,
            )
        } else {
            // Never completed a grid, and nowhere left to look. No solution means no nogood
            // ruling one out, so the grid in hand is still honest about the puzzle.
            Report::from_grid(
                puzzle,
                &self.ll_state.grid,
                self.ll_state.cells_left,
                self.ll_state.solve_counts,
            )
        }
    }
}

// TODO: we should really fix the grid solver to use the cache when trying to skim. Might help performance!

pub fn conprop_solve<C: Clue, K: GridKind>(
    puzzle: &Puzzle<C, K>,
    options: &SolveOptions,
) -> anyhow::Result<Report> {
    let mut line_cache: Option<LineCache<C>> = Some(LineCache::new());

    let options = SolveOptions {
        display_cli_progress: false,
        ..options.clone()
    };

    let mut linear_ctx = SolveContext::new(puzzle, &mut line_cache, &options);
    let mut linear_state = SolveState::new(
        &mut linear_ctx,
        vec![Cell::new(&puzzle.palette); puzzle.geometry.cell_count()].into(),
    );
    linear_state.run_and_check(&mut linear_ctx)?; // `?` because contradictions here are "real"

    if linear_state.cells_left == 0 {
        // let _ = ctx.progress.send(1.0);
        return Ok(linear_state.report(puzzle)); // No fancy stuff required!
    }

    let mut state = ConpropState::new(linear_state);

    loop {
        // Lazily make new pickers so we can do it *after* propagation:
        if state.pickers.len() <= state.guesses_in_trail.len() {
            assert_eq!(
                state.pickers.len(),
                state.guesses_in_trail.len(),
                "We should only ever be one picker short!"
            );
            let new_picker =
                Picker::from_situation(puzzle, &state.ll_state.grid, &state.vsids, &mut state.rng);
            state.pickers.push(new_picker);
        }

        let picker = state.pickers.last_mut().unwrap();
        let Some((cell_idx, color)) = picker.pick(puzzle, &state.ll_state.grid, &state.vsids)
        else {
            assert!(state.guesses_in_trail.is_empty());
            return Ok(state.no_guesses_left(puzzle));
        };

        if linear_ctx.options.trace_backtrack {
            println!("Making guess ({cell_idx:?}, {color:?})");
        }

        if !state
            .make_guess((cell_idx, color), &mut linear_ctx)
            .is_ok_and(|b| b)
        {
            continue; // Skip impossible or already-known picks
        }

        if let Some(report) = state.propagate_and_learn(puzzle, &mut linear_ctx) {
            return Ok(report);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashMap;

    use crate::geometry::Square;
    use crate::puzzle::{BACKGROUND, ColorInfo, Nono};

    /// Three colors and no clues at all, so nothing narrows a cell except what a test says to.
    fn scratch_puzzle() -> Puzzle<Nono, Square> {
        let mut palette = HashMap::new();
        palette.insert(BACKGROUND, ColorInfo::default_bg());
        palette.insert(Color(1), ColorInfo::default_fg(Color(1)));
        palette.insert(Color(2), ColorInfo::default_fg(Color(2)));

        Puzzle::single_lane(palette, 4, vec![])
    }

    /// A state holding one nogood over `literals`, an empty trail, and a blank grid.
    fn scratch_state<'p>(
        puzzle: &'p Puzzle<Nono, Square>,
        ctx: &mut SolveContext<'p, '_, Nono, Square>,
        literals: &[(CellIdx, Color)],
    ) -> ConpropState<'p, Nono> {
        let cell_count = puzzle.geometry.cell_count();
        let mut nogoods_by_cell: TiVec<CellIdx, Vec<usize>> = vec![vec![]; cell_count].into();
        for &(cell, _) in literals {
            nogoods_by_cell[cell].push(0);
        }

        let ll_state = SolveState::new(ctx, vec![Cell::new(&puzzle.palette); cell_count].into());

        let mut res = ConpropState::new(ll_state);
        res.nogoods.push(Nogood {
            not_all_true: literals.iter().copied().collect(),
            current_false_count: 0,
            active: false,
        });
        res.nogoods_by_cell = nogoods_by_cell;

        res
    }

    /// Learn one fact, recording it on the trail the way the search does. It's blamed on lane 0
    /// (the only lane these puzzles have), though nothing here actually derives it.
    fn learn_onto_trail<'p>(
        state: &mut ConpropState<'p, Nono>,
        ctx: &mut SolveContext<'p, '_, Nono, Square>,
        cell: CellIdx,
        is: bool,
        color: Color,
    ) {
        learn_onto_trail_because(state, ctx, cell, is, color, TrailReason::Lane(LaneIdx(0)));
    }

    fn learn_onto_trail_because<'p>(
        state: &mut ConpropState<'p, Nono>,
        ctx: &mut SolveContext<'p, '_, Nono, Square>,
        cell: CellIdx,
        is: bool,
        color: Color,
        reason: TrailReason,
    ) {
        state.trail.push(TrailStep {
            cell_idx: cell,
            old_value: state.ll_state.grid[cell],
            reason,
        });
        assert!(
            state.ll_state.learn(ctx, cell, is, color).unwrap(),
            "the test meant to learn something new about cell {cell:?}"
        );
    }

    /// `update_nogood_counters(n)` means "the counters should describe the grid after the first
    /// `n` entries of the trail" — half-open, so `n` is a length and not an index. Getting that
    /// boundary wrong by one is invisible until a nogood fires a step early or a step late, so
    /// this pins every edge of it: an empty range, one entry at a time, a jump over several, and
    /// the same walk backwards.
    #[test]
    fn nogood_counters_track_the_trail_one_entry_at_a_time() {
        let puzzle = scratch_puzzle();
        let options = SolveOptions::default();
        let mut line_cache = None;
        let mut ctx = SolveContext::new(&puzzle, &mut line_cache, &options);

        // "cell 0 isn't 1, or cell 1 isn't 1, or cell 3 isn't the background."
        let literals = [
            (CellIdx(0), Color(1)),
            (CellIdx(1), Color(1)),
            (CellIdx(3), BACKGROUND),
        ];
        let mut state = scratch_state(&puzzle, &mut ctx, &literals);

        // One trail entry each, in an order that puts a cell the nogood says nothing about
        // (cell 2) in the middle, so a range that runs one too far can't hide behind a hit.
        for (cell, color) in [
            (CellIdx(0), Color(1)),
            (CellIdx(2), Color(2)),
            (CellIdx(1), Color(1)),
            (CellIdx(3), BACKGROUND),
        ] {
            learn_onto_trail(&mut state, &mut ctx, cell, /*is=*/ true, color);
        }

        // Nothing folded in yet: the counter describes a grid where none of this has happened.
        assert_eq!(state.nogoods[0].current_false_count, 0);

        // An empty range changes nothing.
        state.update_nogood_counters(0);
        assert_eq!(state.nogoods[0].current_false_count, 0);
        assert_eq!(state.nogoods_updated_to, 0);

        // `1` takes in entry 0 — cell 0, which the nogood names — and stops before entry 1.
        state.update_nogood_counters(1);
        assert_eq!(
            state.nogoods[0].current_false_count, 1,
            "one entry in, only cell 0 should have counted"
        );
        assert_eq!(state.nogoods_updated_to, 1);

        // Entry 1 is cell 2, which the nogood doesn't name.
        state.update_nogood_counters(2);
        assert_eq!(state.nogoods[0].current_false_count, 1);

        // Entries 2 and 3 are the other two literals, taken in one jump.
        state.update_nogood_counters(4);
        assert_eq!(state.nogoods[0].current_false_count, 3);
        assert_eq!(state.nogoods_updated_to, 4);

        // Rewinding walks the same half-open range the other way. It reads the grid as it stands,
        // so it has to run *before* `SolveState::unwind` puts the cells back.
        state.update_nogood_counters(2);
        assert_eq!(
            state.nogoods[0].current_false_count, 1,
            "rewinding to 2 should undo entries 2 and 3, and no more"
        );
        assert_eq!(state.nogoods_updated_to, 2);

        state.update_nogood_counters(0);
        assert_eq!(state.nogoods[0].current_false_count, 0);
        assert_eq!(state.nogoods_updated_to, 0);
    }

    /// The trail records every narrowing, so one cell can take several entries to become known.
    /// The counter has to move exactly once however the range gets cut up — whether the entries
    /// arrive in two calls or in one.
    ///
    /// The catch-ups are interleaved with the learning here rather than done at the end, because
    /// going forwards only makes sense at the end of the trail: the range says which entries to
    /// look at, but whether a cell counts is read off the grid *as it stands*, and the two only
    /// line up when the grid is as far along as the trail is.
    #[test]
    fn a_cell_narrowed_over_several_entries_counts_once() {
        let puzzle = scratch_puzzle();
        let options = SolveOptions::default();
        let mut line_cache = None;
        let mut ctx = SolveContext::new(&puzzle, &mut line_cache, &options);

        let literals = [(CellIdx(0), Color(1))];
        let mut state = scratch_state(&puzzle, &mut ctx, &literals);

        // {bg,1,2} minus 2 is still unknown, so this entry settles nothing and counts for nothing.
        learn_onto_trail(
            &mut state,
            &mut ctx,
            CellIdx(0),
            /*is=*/ false,
            Color(2),
        );
        assert!(!state.ll_state.grid[CellIdx(0)].is_known());
        state.update_nogood_counters(1);
        assert_eq!(
            state.nogoods[0].current_false_count, 0,
            "cell 0 wasn't known yet after the first narrowing"
        );

        // The second entry is where the cell lands on a color.
        learn_onto_trail(
            &mut state,
            &mut ctx,
            CellIdx(0),
            /*is=*/ true,
            Color(1),
        );
        assert!(state.ll_state.grid[CellIdx(0)].is_known_to_be(Color(1)));
        state.update_nogood_counters(2);
        assert_eq!(state.nogoods[0].current_false_count, 1);

        // Rewinding takes it back out once, not once per entry...
        state.update_nogood_counters(0);
        assert_eq!(
            state.nogoods[0].current_false_count, 0,
            "the two entries for cell 0 were taken back out twice"
        );

        // ...and taking both entries in a single sweep counts it once, not twice.
        state.update_nogood_counters(2);
        assert_eq!(
            state.nogoods[0].current_false_count, 1,
            "the two entries for cell 0 were counted twice"
        );
    }

    /// Make a guess the way the search does: note where it lands on the trail, then learn it.
    fn guess_onto_trail<'p>(
        state: &mut ConpropState<'p, Nono>,
        ctx: &mut SolveContext<'p, '_, Nono, Square>,
        cell: CellIdx,
        color: Color,
    ) {
        state.guesses_in_trail.push((state.trail.len(), color));
        learn_onto_trail_because(
            state,
            ctx,
            cell,
            /*is=*/ true,
            color,
            TrailReason::Guess,
        );
    }

    /// A trail of two guesses, each followed by one consequence, so the guesses sit at trail
    /// entries 0 and 2 and there is an entry on either side of every boundary worth probing.
    fn two_guesses_deep<'p>(
        state: &mut ConpropState<'p, Nono>,
        ctx: &mut SolveContext<'p, '_, Nono, Square>,
    ) {
        guess_onto_trail(state, ctx, CellIdx(0), Color(1));
        learn_onto_trail(state, ctx, CellIdx(1), /*is=*/ true, Color(2));
        guess_onto_trail(state, ctx, CellIdx(2), Color(1));
        learn_onto_trail(state, ctx, CellIdx(3), /*is=*/ true, BACKGROUND);
        state.update_nogood_counters(state.trail.len());
    }

    /// `backjump(k)` rewinds to just *before* guess `k`: guesses `0..k` stay, along with
    /// everything they implied, and guess `k` and everything after it goes. One off in either
    /// direction either strands a guess the search believes it dropped or throws away one it
    /// believes it kept, so this pins both ends — dropping just the last guess, and dropping the
    /// lot — and checks the nogood counters came back with them.
    #[test]
    fn backjump_rewinds_to_just_before_the_guess() {
        let puzzle = scratch_puzzle();
        let options = SolveOptions::default();
        let mut line_cache = None;
        let mut ctx = SolveContext::new(&puzzle, &mut line_cache, &options);

        // One literal on each side of the cut, so a range that runs an entry too far or an entry
        // too short shows up in the count rather than cancelling out.
        let literals = [(CellIdx(0), Color(1)), (CellIdx(3), BACKGROUND)];
        let mut state = scratch_state(&puzzle, &mut ctx, &literals);
        two_guesses_deep(&mut state, &mut ctx);

        assert_eq!(state.guesses_in_trail, vec![(0, Color(1)), (2, Color(1))]);
        assert_eq!(state.trail.len(), 4);
        assert_eq!(state.nogoods[0].current_false_count, 2);

        // Just before guess 1: guess 0 stays, and so does entry 1, the consequence that came of
        // it. Guess 1 (entry 2) and its consequence (entry 3) go.
        state.backjump(1, &mut ctx);
        assert_eq!(
            state.trail.len(),
            2,
            "backjump kept or dropped one trail entry too many"
        );
        assert_eq!(state.guesses_in_trail, vec![(0, Color(1))]);
        assert_eq!(state.nogoods_updated_to, 2);
        assert_eq!(
            state.nogoods[0].current_false_count, 1,
            "cell 3 was rewound past and shouldn't still count; cell 0 wasn't and should"
        );

        // Just before guess 0 is the root: nothing survives.
        state.backjump(0, &mut ctx);
        assert!(state.trail.is_empty());
        assert!(state.guesses_in_trail.is_empty());
        assert_eq!(state.nogoods_updated_to, 0);
        assert_eq!(state.nogoods[0].current_false_count, 0);
    }

    /// You're right that a nogood naming two or more cells is fine: it can only have fired once
    /// the other literals were satisfied, and it fires in the same `propagate` that made it unit,
    /// so its deduction lands in the same guess block. Any rewind that reaches the deduction has
    /// to reach that literal too, and `inform` clears the flag on the way past.
    ///
    /// A nogood naming *one* cell has no other literal to lean on — and `guess_is_nogood` returns one
    /// of those from both of its early exits, whenever a guess turns out to be wrong on its own.
    /// Its deduction says that cell *isn't* a color, which leaves the cell unknown, so
    /// `update_nogood_counters` walks straight past it (`known_or()` is `None`) and `inform` is
    /// never called at all. The flag stays set, the nogood never speaks again, and the picker is
    /// free to walk back into the value it ruled out.
    #[test]
    fn a_one_cell_nogood_comes_back_on_after_its_deduction_is_unwound() {
        let puzzle = scratch_puzzle();
        let options = SolveOptions::default();
        let mut line_cache = None;
        let mut ctx = SolveContext::new(&puzzle, &mut line_cache, &options);

        // "cell 0 is not Color(1)" — what `guess_is_nogood` hands back for a guess that's wrong alone.
        let literals = [(CellIdx(0), Color(1))];
        let mut state = scratch_state(&puzzle, &mut ctx, &literals);

        // A guess about something else, so there's a block for the deduction to live in.
        guess_onto_trail(&mut state, &mut ctx, CellIdx(2), Color(2));

        // Fire the nogood the way `propagate` does.
        assert_eq!(
            state.nogoods[0].deduction(&state.ll_state).unwrap(),
            Some((CellIdx(0), Color(1))),
            "a one-cell nogood with nothing counted against it is unit"
        );
        state.nogoods[0].active = true;
        learn_onto_trail_because(
            &mut state,
            &mut ctx,
            CellIdx(0),
            /*is=*/ false,
            Color(1),
            TrailReason::Nogood(0),
        );
        assert!(
            !state.ll_state.grid[CellIdx(0)].is_known(),
            "still {{bg, 2}}"
        );

        // One more entry after it, so the deduction isn't the last thing on the trail.
        learn_onto_trail(
            &mut state,
            &mut ctx,
            CellIdx(3),
            /*is=*/ true,
            BACKGROUND,
        );
        state.update_nogood_counters(state.trail.len());

        state.backjump(0, &mut ctx);

        assert!(
            state.ll_state.grid[CellIdx(0)].can_be(Color(1)),
            "the deduction was rewound, so cell 0 can be Color(1) again"
        );
        assert!(
            !state.nogoods[0].active,
            "the deduction is gone, so the nogood has to be willing to make it again"
        );
        assert_eq!(
            state.nogoods[0].deduction(&state.ll_state).unwrap(),
            Some((CellIdx(0), Color(1)))
        );
    }

    /// Rewinding the bookkeeping is only half of it: the grid has to go back to what it held
    /// before the guess, or the next guess is made against cells that a discarded branch decided.
    /// `SolveState::unwind` is what does that, from the very trail entries `backjump` drops.
    #[test]
    fn backjump_puts_the_grid_back() {
        let puzzle = scratch_puzzle();
        let options = SolveOptions::default();
        let mut line_cache = None;
        let mut ctx = SolveContext::new(&puzzle, &mut line_cache, &options);

        let literals = [(CellIdx(0), Color(1)), (CellIdx(3), BACKGROUND)];
        let mut state = scratch_state(&puzzle, &mut ctx, &literals);

        guess_onto_trail(&mut state, &mut ctx, CellIdx(0), Color(1));
        learn_onto_trail(
            &mut state,
            &mut ctx,
            CellIdx(1),
            /*is=*/ true,
            Color(2),
        );

        // What the second guess is about to be made from.
        let grid_before = state.ll_state.grid.clone();
        let cells_left_before = state.ll_state.cells_left;

        guess_onto_trail(&mut state, &mut ctx, CellIdx(2), Color(1));
        learn_onto_trail(
            &mut state,
            &mut ctx,
            CellIdx(3),
            /*is=*/ true,
            BACKGROUND,
        );
        state.update_nogood_counters(state.trail.len());

        state.backjump(1, &mut ctx);

        assert_eq!(
            state.ll_state.grid, grid_before,
            "the cells guess 1 settled are still settled after backjumping past it"
        );
        assert_eq!(state.ll_state.cells_left, cells_left_before);
    }

    /// One lane of `len` cells, with a single clue of two `Color(1)`s.
    fn one_clue_puzzle(colors: &[Color], len: usize) -> Puzzle<Nono, Square> {
        let mut palette = HashMap::new();
        palette.insert(BACKGROUND, ColorInfo::default_bg());
        for &color in colors {
            palette.insert(color, ColorInfo::default_fg(color));
        }
        Puzzle::single_lane(
            palette,
            len,
            vec![Nono {
                color: Color(1),
                count: 2,
            }],
        )
    }

    /// `1 . _ . 1` against a clue of `2`: the ends can't both be in the one block. The background
    /// in the middle is on the trail too, but the contradiction doesn't need it, so the nogood
    /// shouldn't name it. And since its two literals come from different guesses, it asserts.
    #[test]
    fn a_lane_nogood_names_only_what_the_contradiction_needs() {
        let puzzle = one_clue_puzzle(&[Color(1)], 5);
        let options = SolveOptions::default();
        let mut line_cache = None;
        let mut ctx = SolveContext::new(&puzzle, &mut line_cache, &options);
        let mut state = scratch_state(&puzzle, &mut ctx, &[]);

        guess_onto_trail(&mut state, &mut ctx, CellIdx(0), Color(1));
        learn_onto_trail(
            &mut state,
            &mut ctx,
            CellIdx(2),
            /*is=*/ true,
            BACKGROUND,
        );
        guess_onto_trail(&mut state, &mut ctx, CellIdx(4), Color(1));
        state.update_nogood_counters(state.trail.len());

        let (nogood, asserting_at) = state.line_is_nogood(LaneIdx(0), &ctx).unwrap();
        assert_eq!(
            nogood.not_all_true,
            HashSet::from([(CellIdx(0), Color(1)), (CellIdx(4), Color(1))])
        );
        assert_eq!(nogood.current_false_count, 2);
        assert_eq!(
            asserting_at,
            Some(1),
            "rewinding guess 1 leaves cell 0 in place, so the nogood forces cell 4"
        );
    }

    /// `{1,2} . 1` against a clue of `2`: the only `Color(1)` block that fits cell 0 is `0..2`,
    /// which misses cell 2. But "cell 0 isn't background" isn't something a literal can say, so
    /// there's no lane nogood to learn.
    #[test]
    fn a_lane_nogood_cant_lean_on_a_merely_narrowed_cell() {
        let puzzle = one_clue_puzzle(&[Color(1), Color(2)], 3);
        let options = SolveOptions::default();
        let mut line_cache = None;
        let mut ctx = SolveContext::new(&puzzle, &mut line_cache, &options);
        let mut state = scratch_state(&puzzle, &mut ctx, &[]);

        guess_onto_trail(&mut state, &mut ctx, CellIdx(2), Color(1));
        learn_onto_trail(
            &mut state,
            &mut ctx,
            CellIdx(0),
            /*is=*/ false,
            BACKGROUND,
        );
        state.update_nogood_counters(state.trail.len());

        assert!(state.line_is_nogood(LaneIdx(0), &ctx).is_none());
    }
}
