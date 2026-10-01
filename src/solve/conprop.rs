use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::mpsc;

use typed_index_collections::TiVec;

use anyhow::bail;
use rand::{SeedableRng, rngs::StdRng};
use web_time::{Duration, Instant};

use crate::{
    geometry::{CellIdx, GridKind, LaneIdx, LanePos},
    gui,
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
    /// The literals. A literal holds if its cell is narrowed to a subset of this cell;
    /// it's a contradiction for all of them to be true at once.
    not_all_true: HashMap<CellIdx, Cell>,
    current_false_count: usize,
}

impl Nogood {
    /// An error if the nogood is already contradicted, `Ok(Some((cell, within)))` if `cell` has
    /// to be narrowed to `within`.
    fn deduction<C: Clue>(
        &self,
        ll_state: &SolveState<C>,
    ) -> anyhow::Result<Option<(CellIdx, Cell)>> {
        let remaining = self.not_all_true.len() - self.current_false_count;
        if remaining > 1 {
            return Ok(None);
        }
        if remaining == 0 {
            debug_assert!(
                self.not_all_true
                    .iter()
                    .all(|(&cell_idx, &within)| ll_state.grid[cell_idx].is_within(within)),
                "Claude was right to be worried about this"
            );
            bail!("Nogood contradicted")
        }

        for (&cell_idx, &within) in &self.not_all_true {
            let cell = ll_state.grid[cell_idx];
            if !cell.is_within(within) {
                let rest = cell.without(within);
                return Ok((rest != cell).then_some((cell_idx, rest)));
            }
        }
        bail!("Nogood contradicted, and `current_false_count` was stale");
    }

    /// Between the two ends of a range of the trail, `cell_idx` went from `low` to `high`.
    fn inform(&mut self, cell_idx: CellIdx, low: Cell, high: Cell, forwards: bool) {
        // `nogoods_by_cell` should mean that `cell_idx` is always relevant
        let within = self.not_all_true[&cell_idx];

        if !low.is_within(within) && high.is_within(within) {
            if forwards {
                self.current_false_count += 1;
                debug_assert!(self.current_false_count <= self.not_all_true.len());
            } else {
                self.current_false_count = self.current_false_count.strict_sub(1);
            }
        }
    }
}

// TODO: there are a bunch of indices; maybe type them, too.
pub struct ConpropState<'p, 'c, 'x, C: Clue, K: GridKind> {
    ctx: &'c mut SolveContext<'p, 'x, C, K>,
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

    // TODO: feed this to the picker, so it only picks things that contradict this
    solution_found: Option<PartialSolution>, // never actually "Partial", of course.
    /// Everything we know at the root (without assumptions) when a first solution is found.
    /// (The search for a second solution requires assuming the first one is wrong).
    root_knowledge: Option<(PartialSolution, usize)>,
}

impl<'p, 'c, 'x, C: Clue, K: GridKind> ConpropState<'p, 'c, 'x, C, K> {
    pub fn new(ctx: &'c mut SolveContext<'p, 'x, C, K>, ll_state: SolveState<'p, C>) -> Self {
        ConpropState {
            ctx,
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
            solution_found: None,
            root_knowledge: None,
        }
    }

    /// A copy to experiment on. Because it borrows `ctx`, the original is unusable while this remains.
    /// This clears `pickers`, since the experiment will control the picking.
    fn fork_to_root(&mut self) -> ConpropState<'p, '_, 'x, C, K> {
        let mut new_state = ConpropState {
            ctx: &mut *self.ctx,
            nogoods: self.nogoods.clone(),
            nogoods_by_cell: self.nogoods_by_cell.clone(),
            vsids: HashMap::default(), // vsids only matters for pickers
            vsids_decay: 0,
            // TODO: make `update_nogood_counters` read `self`'s trail; skip cloning it:
            trail: self.trail.clone(),
            nogoods_updated_to: self.nogoods_updated_to,
            guesses_in_trail: self.guesses_in_trail.clone(), // (and this one)
            pickers: vec![],                                 // guesses will be made manually
            rng: self.rng.clone(),
            ll_state: self.ll_state.clone(),
            solution_found: None, // irrelevant
            root_knowledge: None, // irrelevant
        };

        new_state.backjump(0);
        new_state
    }

    // Typically, you'll call `propagate{,_and_learn}` after this.
    // `Err(_)` if the guess is inherently wrong, `Ok(false)` if the guess was already true.
    // Only makes changes if it returns `Ok(true)`
    // TODO: maybe just return a bool?
    fn make_guess(&mut self, (cell_idx, color): (CellIdx, Color)) -> anyhow::Result<bool> {
        let old_cell = self.ll_state.grid[cell_idx];
        let res = self
            .ll_state
            .learn(self.ctx, cell_idx, /*is*/ true, color)?;
        if res {
            self.guesses_in_trail.push((self.trail.len(), color));
            // TODO: fold the `trail` update in `.learn_new`
            self.trail.push(TrailStep {
                cell_idx,
                old_value: old_cell,
                level: self.guesses_in_trail.len(),
                reason: TrailReason::Guess,
            });
        }

        Ok(res)
    }

    /// Rewind to just *before* `guess_idx` (including its consequences)...
    /// This pops stuff off `trail` and undoes progress towards `nogoods`
    fn backjump(&mut self, guess_idx: usize) {
        if self.guesses_in_trail.is_empty() {
            assert_eq!(guess_idx, 0); // We do `backjump(0)` without checking
            return;
        }
        debug_assert_eq!(self.nogoods_updated_to, self.trail.len());

        let (trail_idx, _guess) = self.guesses_in_trail[guess_idx];
        self.update_nogood_counters(trail_idx);
        self.ll_state.unwind(self.ctx, &self.trail[trail_idx..]);

        self.trail.truncate(trail_idx);
        self.guesses_in_trail.truncate(guess_idx);
        self.pickers.truncate(guess_idx + 1);
    }

    /// Start watching a nogood.
    fn add_nogood(&mut self, nogood: Nogood) {
        let nogood_idx = self.nogoods.len();
        for (&cell_idx, within) in &nogood.not_all_true {
            self.nogoods_by_cell[cell_idx].push(nogood_idx);

            // Guessing any of these makes the literal true.
            for color in within.can_be_iter() {
                *self.vsids.entry((cell_idx, color)).or_insert(0.0) += 1.0;
            }
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
        let not_all_true: HashMap<CellIdx, Cell> = self
            .guesses_in_trail
            .iter()
            .map(|&(trail_idx, color)| (self.trail[trail_idx].cell_idx, Cell::from_color(color)))
            .collect();

        Nogood {
            current_false_count: not_all_true.len(), // every one of them holds right now
            not_all_true,
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
            // Don't double-count! All we're doing here is comparing the two ends of the range, and
            // the first entry for a cell has its value at the low end. The grid has the high end.
            if !cells_seen.insert(cell_idx) {
                continue;
            }
            if old_value.is_known() {
                panic!("Made a redundant guess: {cell_idx:?} {old_value:?}"); // `continue` would be safe.
            }

            let high = self.ll_state.grid[cell_idx];
            for &nogood_idx in &self.nogoods_by_cell[cell_idx] {
                self.nogoods[nogood_idx].inform(cell_idx, old_value, high, forwards);
            }
        }

        self.nogoods_updated_to = new_idx;
    }

    /// Applies linear logic and nogoods until everything possible is deduced. `Ok(true)` if a solution is found.
    ///
    /// On a `Conflict`, the grid is left as the conflict found it, so explain it before unwinding.
    fn propagate(&mut self) -> Result<bool, Conflict> {
        let mut any_nogoods_fired = true;
        while any_nogoods_fired {
            any_nogoods_fired = false;

            let linear_res = self
                .ll_state
                .run_and_check_recording(self.ctx, &mut self.trail);
            self.update_nogood_counters(self.trail.len());

            // Had to update the counters first.
            linear_res.map_err(|contradiction| Conflict::Lane(contradiction.lane))?;

            for nogood_idx in 0..self.nogoods.len() {
                self.update_nogood_counters(self.trail.len()); // Get it right before `deduction`.

                let nogood = &mut self.nogoods[nogood_idx];

                let deduction = nogood
                    .deduction(&self.ll_state)
                    .map_err(|_| Conflict::Nogood(nogood_idx))?;
                if let Some((cell_idx, within)) = deduction {
                    any_nogoods_fired = true;

                    let before = self.ll_state.grid[cell_idx];
                    // An error here means the implication of the nogood contradicts what we
                    // already know.
                    let learned = self
                        .ll_state
                        .learn_within(self.ctx, cell_idx, within)
                        .map_err(|_| Conflict::Nogood(nogood_idx))?;
                    if learned {
                        // Only when it actually moved: a no-op entry is a cell the trail claims
                        // changed when it didn't, and `update_nogood_counters` believes the trail.
                        self.trail.push(TrailStep {
                            cell_idx,
                            old_value: before,
                            level: self.guesses_in_trail.len(),
                            reason: TrailReason::Nogood(nogood_idx),
                        });
                    }
                }
            }
        }

        Ok(self.ll_state.cells_left == 0)
    }

    /// `Ok(Some(_))` if the search is over, `Err(_)` if there's no solution.
    fn propagate_and_learn(&mut self) -> anyhow::Result<Option<Report>> {
        let puzzle = self.ctx.puzzle;
        let mut run_res = self.propagate();
        while let Err(conflict) = run_res {
            if self.guesses_in_trail.is_empty() {
                // Root is contradictory!
                if self.solution_found.is_some() {
                    return Ok(Some(self.no_guesses_left())); // ...so the first solution is unique
                }
                bail!("puzzle has no solutions"); // ...so there's no solution at all
            }

            let (nogood, asserting_at) = match conflict {
                // Get a nogood from the line where the conflict happened:
                Conflict::Lane(lane_idx) => self.line_is_nogood(lane_idx),
                // We can instead make a nogood from the guesses:
                Conflict::Nogood(_) => self.guess_is_nogood(),
            };

            self.add_nogood(nogood);
            self.backjump(asserting_at);

            run_res = self.propagate()
        }
        if run_res.unwrap() {
            // We found a valid solution!
            if let Some(existing_solution) = &self.solution_found {
                assert!(
                    *existing_solution != self.ll_state.grid,
                    "TODO: I thought we couldn't reach the same solution multiple times"
                );

                // Multiple valid solutions: report our snapshot of the cells we proved.
                let (grid, cells_left) = self.root_knowledge.as_ref().unwrap();
                return Ok(Some(Report::from_grid(
                    puzzle,
                    grid,
                    *cells_left,
                    self.ll_state.solve_counts,
                )));
            }
            // Record first solution:
            self.solution_found = Some(self.ll_state.grid.clone());

            if self.ctx.options.stop_at_first_solution {
                // `cells_left` is a bit of a lie, since we don't know the solution is unique
                return Ok(Some(Report::from_grid(
                    puzzle,
                    &self.ll_state.grid,
                    /*cells_left=*/ 0,
                    self.ll_state.solve_counts,
                )));
            }

            if self.guesses_in_trail.is_empty() {
                // Nice, no guesses outstanding. We know it's unique.
                return Ok(Some(Report::from_grid(
                    puzzle,
                    &self.ll_state.grid,
                    /*cells_left=*/ 0,
                    self.ll_state.solve_counts,
                )));
            }

            // Move the goalposts: now try to find a second solution.
            let first_solution_is_nogood = self.solution_nogood();
            self.add_nogood(first_solution_is_nogood);

            // Now go back and try again!
            self.backjump(0);
            // Record what we know without any assumptions (and before using the fake nogood)
            self.root_knowledge = Some((self.ll_state.grid.clone(), self.ll_state.cells_left));

            return self.propagate_and_learn();
        } else {
            return Ok(None); // No solution, no error.
        }
    }

    /// Given that `probe` is contradictory, change as many entries as possible to `root` while
    /// keeping it contradictory.
    /// Returns indices that had to be kept because they were load-bearing.
    fn minimize_line_contradiction(probe: &mut [Cell], root: &[Cell], clues: &[C]) -> Vec<usize> {
        let contradicts = |lane: &[Cell]| exhaust_line(clues, &mut lane.to_vec()).is_err();

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
        debug_assert!(!kept.is_empty(), "{root:?} contradicts at the root");

        kept
    }

    /// Why can't `lane_idx` be satisfied, as it stood just before trail entry `before`?
    /// (`self.trail.len()` is also valid for the whole thing.) The answer is a minimal set of
    /// literals.
    ///
    /// With `pinned: Some((cell_idx, within))`, it instead explains why entry `before` (which
    /// must be for `cell_idx`) narrowed it to `within`. The answer only names `cell_idx` if what
    /// was already known about it was part of the reason.
    fn lane_reason(
        &self,
        lane_idx: LaneIdx,
        before: usize,
        pinned: Option<(CellIdx, Cell)>,
    ) -> Vec<(CellIdx, Cell)> {
        let lane_cells = &self.ctx.lane_map().lanes[lane_idx].cells;
        let clues = self.ll_state.lanes[lane_idx].clues;

        let contradicts = |lane: &[Cell]| exhaust_line(clues, &mut lane.to_vec()).is_err();

        // TODO: why isn't `first_guess_at` always 0?
        let first_guess_at = self.first_guess_at();
        let lane_pos_of: HashMap<CellIdx, usize> = lane_cells
            .iter()
            .enumerate()
            .map(|(pos, &cell_idx)| (cell_idx, pos))
            .collect();

        // Rewind time using the trail: `probe` stops at `before`, and `root` goes all the way
        // back to the first guess.
        let mut probe = vec![];
        // TODO: we `pub`ed `gather_into` and `.clue` just for this function; can we be less ad-hoc?
        gather_into(
            self.ctx.lane_map(),
            lane_idx,
            &self.ll_state.grid,
            &mut probe,
        );
        let mut root = probe.clone();
        for trail_idx in (first_guess_at..self.trail.len()).rev() {
            let step = &self.trail[trail_idx];
            if let Some(&pos) = lane_pos_of.get(&step.cell_idx) {
                root[pos] = step.old_value;
                if trail_idx >= before {
                    probe[pos] = step.old_value;
                }
            }
        }

        // Suppose the pinned cell is anything *but* `within`. Minimization will find out whether
        // what was already known about it matters.
        let pinned = pinned.map(|(cell_idx, within)| {
            let pos = lane_pos_of[&cell_idx];
            let already_known = probe[pos];
            debug_assert!(
                !already_known.is_within(within),
                "{cell_idx:?} was already within {within:?}; why explain it now?"
            );
            probe[pos] = already_known.without(within);
            root[pos] = root[pos].without(within);
            (pos, already_known)
        });

        debug_assert!(
            contradicts(&probe),
            "{lane_idx:?} was blamed (for {pinned:?}), but it's satisfiable"
        );

        let kept = Self::minimize_line_contradiction(&mut probe, &root, clues);
        kept.into_iter()
            .map(|pos| {
                let literal = match pinned {
                    // `probe` only holds what we supposed about it.
                    Some((pinned_pos, already_known)) if pos == pinned_pos => already_known,
                    _ => probe[pos],
                };
                (lane_cells[LanePos::from(pos)], literal)
            })
            .collect()
    }

    /// The trail index where the search starts making assumptions.
    fn first_guess_at(&self) -> usize {
        self.guesses_in_trail
            .first()
            .map_or(self.trail.len(), |&(trail_idx, _)| trail_idx)
    }

    /// We have a contradiction we can make into a nogood, but we'd like it to be *asserting*;
    /// we need exactly one of its literals to be after the most recent guess, so we can backjump
    /// past it, proving the guess false.
    ///
    /// So, until that's the case, pull off the most recent literal and replace it with the things
    /// that implied it.
    ///
    /// Returns the resulting clause and the level to backjump to.
    fn resolve_to_uip(&self, clause: &[(CellIdx, Cell)]) -> (HashMap<CellIdx, Cell>, usize) {
        // For each cell narrowed since the root, the trail entries that did it, oldest-first.
        let mut entries: HashMap<CellIdx, Vec<usize>> = HashMap::new();
        for trail_idx in self.first_guess_at()..self.trail.len() {
            entries
                .entry(self.trail[trail_idx].cell_idx)
                .or_default()
                .push(trail_idx);
        }
        let root_value = |cell_idx: CellIdx| match entries.get(&cell_idx) {
            Some(steps) => self.trail[steps[0]].old_value,
            None => self.ll_state.grid[cell_idx],
        };

        // The first trail entry after which the literal held, or `None` if it held at the root
        let settled = |cell_idx: CellIdx, within: Cell| -> Option<usize> {
            if root_value(cell_idx).is_within(within) {
                return None;
            }
            let steps = &entries[&cell_idx];
            // Each entry leaves the cell however the next one found it (or the grid, at the end).
            let values_after = (steps[1..].iter())
                .map(|&trail_idx| self.trail[trail_idx].old_value)
                .chain([self.ll_state.grid[cell_idx]]);
            let (&trail_idx, _) = (steps.iter().zip(values_after))
                .find(|(_, value)| value.is_within(within))
                .expect("every literal being resolved holds right now");
            Some(trail_idx)
        };

        // Keyed by the trail entry that settled each literal, oldest-first. At most one per cell.
        let mut literals: BTreeMap<usize, (CellIdx, Cell)> = BTreeMap::new();
        let add_literal = |literals: &mut BTreeMap<usize, (CellIdx, Cell)>,
                           cell_idx: CellIdx,
                           mut within: Cell| {
            // Two literals about the same cell are the same as one about both.
            let existing = literals
                .iter()
                .find(|(_, (other_cell, _))| *other_cell == cell_idx)
                .map(|(&trail_idx, &(_, other_within))| (trail_idx, other_within));
            if let Some((trail_idx, other_within)) = existing {
                literals.remove(&trail_idx);
                within
                    .learn_intersect(other_within)
                    .expect("both literals hold right now, so they overlap");
            }
            if let Some(trail_idx) = settled(cell_idx, within) {
                literals.insert(trail_idx, (cell_idx, within));
            }
        };
        for &(cell_idx, within) in clause {
            add_literal(&mut literals, cell_idx, within);
        }

        let backjump_to = loop {
            let mut deepest_first = literals.iter().rev();
            let Some((&last_idx, &(cell_idx, within))) = deepest_first.next() else {
                break 0; // The contradiction doesn't depend on any guesses at all!
            };
            let deepest = self.trail[last_idx].level;
            // Is the deepest literal alone on its level? If so, we're done!
            match deepest_first.next() {
                None => break 0,
                Some((&prev_idx, _)) if self.trail[prev_idx].level < deepest => {
                    break self.trail[prev_idx].level;
                }
                _ => {}
            }

            // Replace the newest literal with things that imply it.
            literals.remove(&last_idx);
            let reason = match self.trail[last_idx].reason {
                TrailReason::Guess => {
                    unreachable!("a guess is the oldest entry at its level, so it's alone there")
                }
                TrailReason::Nogood(nogood_idx) => {
                    let not_all_true = &self.nogoods[nogood_idx].not_all_true;
                    let mut reason: Vec<(CellIdx, Cell)> = not_all_true
                        .iter()
                        .map(|(&other_cell, &other_within)| (other_cell, other_within))
                        .filter(|&(other_cell, _)| other_cell != cell_idx)
                        .collect();
                    // The nogood only ruled out its own literal. If that's not enough to get
                    // within `within`, what was already known about the cell is a reason, too.
                    let ruled_out = not_all_true[&cell_idx];
                    if !root_value(cell_idx).without(ruled_out).is_within(within) {
                        reason.push((cell_idx, self.trail[last_idx].old_value));
                    }
                    reason
                }
                TrailReason::Lane(lane_idx) => {
                    self.lane_reason(lane_idx, last_idx, Some((cell_idx, within)))
                }
            };

            for (cell_idx, within) in reason {
                add_literal(&mut literals, cell_idx, within);
            }
            if literals
                .last_key_value()
                .is_some_and(|(&idx, _)| idx >= last_idx)
            {
                panic!("Time-travel logic; we're in a loop!"); // But returning `None` would be safe.
            }
        };

        (
            literals.into_values().collect::<HashMap<CellIdx, Cell>>(),
            backjump_to,
        )
    }

    /// Turn a contradiction derived at `lane_idx` into a nogood.
    /// Also returns the backjump destination implied by the nogood.
    ///
    /// This should be called with the contradiction still in the grid.
    fn line_is_nogood(&self, lane_idx: LaneIdx) -> (Nogood, usize) {
        let clause = self.lane_reason(lane_idx, self.trail.len(), None);

        // Now we know what the literals are. Time for 1UIP!
        let (not_all_true, asserting_at) = self.resolve_to_uip(&clause);

        let nogood = Nogood {
            current_false_count: not_all_true.len(), // every one of them holds right now
            not_all_true,
        };
        (nogood, asserting_at)
    }

    /// Minimizes the tail of the `Nogood`, and returns an index (to `guesses_in_trail`) to backjump past,
    /// or `None` if all the guesses should be cleared.
    fn guess_is_nogood(&mut self) -> (Nogood, usize) {
        let mut res = Nogood {
            not_all_true: HashMap::new(),
            current_false_count: 0,
        };

        let Some((last_guess, pfx_guesses)) = self.guesses_in_trail.split_last() else {
            return (res, 0); // No guesses, so the puzzle is contradictory.
        };
        // Last guess first, then the rest in order.
        let replay: Vec<(CellIdx, Color)> = std::iter::once(last_guess)
            .chain(pfx_guesses.iter())
            .map(|&(guess_idx_in_trail, color)| (self.trail[guess_idx_in_trail].cell_idx, color))
            .collect();

        // TODO: the fork is expensive: we might want to try reusing `self` (and restoring it after)
        let mut exp_state = self.fork_to_root();
        exp_state.backjump(0); // Back to the root, to try a different order

        for (after_guess_idx, (cell_idx, color)) in replay.into_iter().enumerate() {
            if exp_state.ll_state.grid[cell_idx].is_known_to_be(color) {
                continue; // no need to guess what we already know
            }
            // if it's known to be another color, we will get a useful contradiction in `.make_guess`

            res.not_all_true.insert(cell_idx, Cell::from_color(color));
            res.current_false_count += 1;

            let guess_ok =
                exp_state.make_guess((cell_idx, color)).is_ok() && exp_state.propagate().is_ok();

            if !guess_ok {
                return (res, after_guess_idx);
            }
        }
        panic!("Should've re-found that contradiction");
    }

    /// Make one guess and then do `propagate_and_learn`.
    fn guess_once(&mut self) -> anyhow::Result<Option<Report>> {
        let puzzle = self.ctx.puzzle;
        // Lazily make new pickers so we can do it *after* propagation:
        if self.pickers.len() <= self.guesses_in_trail.len() {
            assert_eq!(
                self.pickers.len(),
                self.guesses_in_trail.len(),
                "We should only ever be one picker short!"
            );
            let new_picker =
                Picker::from_situation(puzzle, &self.ll_state.grid, &self.vsids, &mut self.rng);
            self.pickers.push(new_picker);
        }

        let picker = self.pickers.last_mut().unwrap();
        let Some((cell_idx, color)) = picker.pick(puzzle, &self.ll_state.grid, &self.vsids) else {
            assert!(self.guesses_in_trail.is_empty());
            return Ok(Some(self.no_guesses_left()));
        };

        if self.ctx.options.trace_backtrack {
            println!("Making guess ({cell_idx:?}, {color:?})");
        }

        if !self.make_guess((cell_idx, color)).is_ok_and(|b| b) {
            return Ok(None); // Skip impossible or already-known picks
        }

        self.propagate_and_learn()
    }

    /// We have tried everything.
    /// (The trail isn't necessarily empty when this is called: a root-level nogood deduction is
    /// on it too, and it stays there when the last guess comes off.)
    fn no_guesses_left(&self) -> Report {
        let puzzle = self.ctx.puzzle;
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

/// Get the state after line logic is done.
fn initial_state<'p, C: Clue, K: GridKind>(
    ctx: &mut SolveContext<'p, '_, C, K>,
) -> anyhow::Result<SolveState<'p, C>> {
    let puzzle = ctx.puzzle;
    let mut linear_state = SolveState::new(
        ctx,
        vec![Cell::new(&puzzle.palette); puzzle.geometry.cell_count()].into(),
    );
    linear_state.run_and_check(ctx)?;
    Ok(linear_state)
}

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
    let linear_state = initial_state(&mut linear_ctx)?;
    if linear_state.cells_left == 0 {
        return Ok(linear_state.report(puzzle)); // No fancy stuff required!
    }

    let mut state = ConpropState::new(&mut linear_ctx, linear_state);
    loop {
        if let Some(report) = state.guess_once()? {
            return Ok(report);
        }
    }
}

/// `conprop_solve`, for running in the background in a GUI. It reports progress and yeilds occasionally.
pub async fn conprop_solve_in_background<C: Clue, K: GridKind>(
    puzzle: &Puzzle<C, K>,
    options: &SolveOptions,
    progress: mpsc::Sender<f32>,
    terminate: mpsc::Receiver<()>,
) -> anyhow::Result<Report> {
    const TIME_BETWEEN_YIELDS: Duration = Duration::from_millis(20);

    let mut line_cache: Option<LineCache<C>> = Some(LineCache::new());

    let options = SolveOptions {
        display_cli_progress: false,
        ..options.clone()
    };

    let total_cells = puzzle.geometry.cell_count();
    // Never exactly 0.0: the GUI takes that to mean nothing is running. (TODO?)
    let report_progress = |cells_left: usize| {
        let done = (total_cells - cells_left) as f32 / total_cells as f32;
        let _ = progress.send(done.max(0.01));
    };

    // Any finish, even an error, has to say so, or the GUI will think it's still running (TODO).
    let finished = || {
        let _ = progress.send(1.0);
    };

    let mut linear_ctx = SolveContext::new(puzzle, &mut line_cache, &options);
    let linear_state = initial_state(&mut linear_ctx).inspect_err(|_| finished())?;
    if linear_state.cells_left == 0 {
        finished();
        return Ok(linear_state.report(puzzle));
    }

    let mut root_cells_left = linear_state.cells_left;
    report_progress(root_cells_left);

    let mut state = ConpropState::new(&mut linear_ctx, linear_state);
    let mut last_yield = Instant::now();
    loop {
        // Only what's known with no guesses outstanding is progress: anything else might still be
        // unwound. And once a first solution is in, even the root assumes it's the wrong one.
        if state.guesses_in_trail.is_empty()
            && state.solution_found.is_none()
            && state.ll_state.cells_left < root_cells_left
        {
            root_cells_left = state.ll_state.cells_left;
            report_progress(root_cells_left);
        }

        if last_yield.elapsed() > TIME_BETWEEN_YIELDS {
            gui::yield_now().await;
            if terminate.try_recv().is_ok() {
                anyhow::bail!("search cancelled");
            }
            last_yield = Instant::now();
        }

        if let Some(outcome) = state.guess_once().transpose() {
            finished();
            return outcome;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashMap;

    use crate::geometry::{Geometry, Outline, Rect, Square, Tri};
    use crate::import::{bw_palette, solution_to_puzzle, solution_to_tri_puzzle};
    use crate::puzzle::{BACKGROUND, ClueStyle, ColorInfo, Nono, Solution};

    /// Three colors and no clues at all, so nothing narrows a cell except what a test says to.
    fn scratch_puzzle() -> Puzzle<Nono, Square> {
        let mut palette = HashMap::new();
        palette.insert(BACKGROUND, ColorInfo::default_bg());
        palette.insert(Color(1), ColorInfo::default_fg(Color(1)));
        palette.insert(Color(2), ColorInfo::default_fg(Color(2)));

        Puzzle::single_lane(palette, 4, vec![])
    }

    /// A state holding one nogood over `literals` (each saying its cell is that color), an empty
    /// trail, and a blank grid.
    fn scratch_state<'p, 'c, 'x>(
        puzzle: &'p Puzzle<Nono, Square>,
        ctx: &'c mut SolveContext<'p, 'x, Nono, Square>,
        literals: &[(CellIdx, Color)],
    ) -> ConpropState<'p, 'c, 'x, Nono, Square> {
        let literals: Vec<(CellIdx, Cell)> = literals
            .iter()
            .map(|&(cell, color)| (cell, Cell::from_color(color)))
            .collect();
        partial_scratch_state(puzzle, ctx, &literals)
    }

    /// `scratch_state`, but the literals can leave their cells unknown.
    fn partial_scratch_state<'p, 'c, 'x>(
        puzzle: &'p Puzzle<Nono, Square>,
        ctx: &'c mut SolveContext<'p, 'x, Nono, Square>,
        literals: &[(CellIdx, Cell)],
    ) -> ConpropState<'p, 'c, 'x, Nono, Square> {
        let cell_count = puzzle.geometry.cell_count();
        let mut nogoods_by_cell: TiVec<CellIdx, Vec<usize>> = vec![vec![]; cell_count].into();
        for &(cell, _) in literals {
            nogoods_by_cell[cell].push(0);
        }

        let ll_state = SolveState::new(ctx, vec![Cell::new(&puzzle.palette); cell_count].into());

        let mut res = ConpropState::new(ctx, ll_state);
        res.nogoods.push(Nogood {
            not_all_true: literals.iter().copied().collect(),
            current_false_count: 0,
        });
        res.nogoods_by_cell = nogoods_by_cell;

        res
    }

    /// Learn one fact, recording it on the trail the way the search does. It's blamed on lane 0
    /// (the only lane these puzzles have), though nothing here actually derives it.
    fn learn_onto_trail(
        state: &mut ConpropState<Nono, Square>,
        cell: CellIdx,
        is: bool,
        color: Color,
    ) {
        learn_onto_trail_because(state, cell, is, color, TrailReason::Lane(LaneIdx(0)));
    }

    fn learn_onto_trail_because(
        state: &mut ConpropState<Nono, Square>,
        cell: CellIdx,
        is: bool,
        color: Color,
        reason: TrailReason,
    ) {
        state.trail.push(TrailStep {
            cell_idx: cell,
            old_value: state.ll_state.grid[cell],
            level: state.guesses_in_trail.len(), // it should be updated first!
            reason,
        });
        assert!(
            state.ll_state.learn(state.ctx, cell, is, color).unwrap(),
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
            learn_onto_trail(&mut state, cell, /*is=*/ true, color);
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
        learn_onto_trail(&mut state, CellIdx(0), /*is=*/ false, Color(2));
        assert!(!state.ll_state.grid[CellIdx(0)].is_known());
        state.update_nogood_counters(1);
        assert_eq!(
            state.nogoods[0].current_false_count, 0,
            "cell 0 wasn't known yet after the first narrowing"
        );

        // The second entry is where the cell lands on a color.
        learn_onto_trail(&mut state, CellIdx(0), /*is=*/ true, Color(1));
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

    /// A literal that doesn't settle its cell holds as soon as the cell is narrowed into it, while
    /// the cell is still unknown — and narrowing it further doesn't make it hold any harder.
    #[test]
    fn a_partial_literal_counts_before_its_cell_is_known() {
        let puzzle = scratch_puzzle();
        let options = SolveOptions::default();
        let mut line_cache = None;
        let mut ctx = SolveContext::new(&puzzle, &mut line_cache, &options);

        // "Not both: cell 0 isn't background, and cell 1 is."
        let not_bg = Cell::new(&puzzle.palette).without(Cell::from_color(BACKGROUND));
        let literals = [
            (CellIdx(0), not_bg),
            (CellIdx(1), Cell::from_color(BACKGROUND)),
        ];
        let mut state = partial_scratch_state(&puzzle, &mut ctx, &literals);

        // {bg,1,2} minus the background is {1,2}: still unknown, but within the literal.
        learn_onto_trail(&mut state, CellIdx(0), /*is=*/ false, BACKGROUND);
        assert!(!state.ll_state.grid[CellIdx(0)].is_known());
        state.update_nogood_counters(1);
        assert_eq!(state.nogoods[0].current_false_count, 1);
        assert_eq!(
            state.nogoods[0].deduction(&state.ll_state).unwrap(),
            Some((CellIdx(1), not_bg)),
            "one literal holds, so cell 1 can't be background"
        );

        // Settling it doesn't count it again.
        learn_onto_trail(&mut state, CellIdx(0), /*is=*/ false, Color(2));
        assert!(state.ll_state.grid[CellIdx(0)].is_known_to_be(Color(1)));
        state.update_nogood_counters(2);
        assert_eq!(state.nogoods[0].current_false_count, 1);

        // And rewinding takes it back out at the entry that put it in, not the one that settled it.
        state.update_nogood_counters(1);
        assert_eq!(state.nogoods[0].current_false_count, 1);
        state.update_nogood_counters(0);
        assert_eq!(state.nogoods[0].current_false_count, 0);
    }

    /// Make a guess the way the search does: note where it lands on the trail, then learn it.
    fn guess_onto_trail(state: &mut ConpropState<Nono, Square>, cell: CellIdx, color: Color) {
        state.guesses_in_trail.push((state.trail.len(), color));
        learn_onto_trail_because(state, cell, /*is=*/ true, color, TrailReason::Guess);
    }

    /// A trail of two guesses, each followed by one consequence, so the guesses sit at trail
    /// entries 0 and 2 and there is an entry on either side of every boundary worth probing.
    fn two_guesses_deep(state: &mut ConpropState<Nono, Square>) {
        guess_onto_trail(state, CellIdx(0), Color(1));
        learn_onto_trail(state, CellIdx(1), /*is=*/ true, Color(2));
        guess_onto_trail(state, CellIdx(2), Color(1));
        learn_onto_trail(state, CellIdx(3), /*is=*/ true, BACKGROUND);
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
        two_guesses_deep(&mut state);

        assert_eq!(state.guesses_in_trail, vec![(0, Color(1)), (2, Color(1))]);
        assert_eq!(state.trail.len(), 4);
        assert_eq!(state.nogoods[0].current_false_count, 2);

        // Just before guess 1: guess 0 stays, and so does entry 1, the consequence that came of
        // it. Guess 1 (entry 2) and its consequence (entry 3) go.
        state.backjump(1);
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
        state.backjump(0);
        assert!(state.trail.is_empty());
        assert!(state.guesses_in_trail.is_empty());
        assert_eq!(state.nogoods_updated_to, 0);
        assert_eq!(state.nogoods[0].current_false_count, 0);
    }

    /// A nogood naming *one* cell — what `guess_is_nogood` hands back whenever a guess is wrong on
    /// its own — has no other literal whose rewinding could tell it to speak up again. Its
    /// deduction says that cell *isn't* a color, which leaves the cell unknown. So once that
    /// deduction is unwound, the nogood has to notice by itself that it's no longer satisfied, or
    /// the picker is free to walk back into the value it ruled out.
    #[test]
    fn a_one_cell_nogood_comes_back_on_after_its_deduction_is_unwound() {
        let puzzle = scratch_puzzle();
        let options = SolveOptions::default();
        let mut line_cache = None;
        let mut ctx = SolveContext::new(&puzzle, &mut line_cache, &options);

        // "cell 0 is not Color(1)" — what `guess_is_nogood` hands back for a guess that's wrong alone.
        let literals = [(CellIdx(0), Color(1))];
        let mut state = scratch_state(&puzzle, &mut ctx, &literals);
        let anything_but_1 = Cell::new(&puzzle.palette).without(Cell::from_color(Color(1)));

        // A guess about something else, so there's a block for the deduction to live in.
        guess_onto_trail(&mut state, CellIdx(2), Color(2));

        // Fire the nogood the way `propagate` does.
        assert_eq!(
            state.nogoods[0].deduction(&state.ll_state).unwrap(),
            Some((CellIdx(0), anything_but_1)),
            "a one-cell nogood with nothing counted against it is unit"
        );
        learn_onto_trail_because(
            &mut state,
            CellIdx(0),
            /*is=*/ false,
            Color(1),
            TrailReason::Nogood(0),
        );
        assert!(
            !state.ll_state.grid[CellIdx(0)].is_known(),
            "still {{bg, 2}}"
        );
        assert_eq!(
            state.nogoods[0].deduction(&state.ll_state).unwrap(),
            None,
            "it already said its piece"
        );

        // One more entry after it, so the deduction isn't the last thing on the trail.
        learn_onto_trail(&mut state, CellIdx(3), /*is=*/ true, BACKGROUND);
        state.update_nogood_counters(state.trail.len());

        state.backjump(0);

        assert!(
            state.ll_state.grid[CellIdx(0)].can_be(Color(1)),
            "the deduction was rewound, so cell 0 can be Color(1) again"
        );
        assert_eq!(
            state.nogoods[0].deduction(&state.ll_state).unwrap(),
            Some((CellIdx(0), anything_but_1)),
            "the deduction is gone, so the nogood has to be willing to make it again"
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

        guess_onto_trail(&mut state, CellIdx(0), Color(1));
        learn_onto_trail(&mut state, CellIdx(1), /*is=*/ true, Color(2));

        // What the second guess is about to be made from.
        let grid_before = state.ll_state.grid.clone();
        let cells_left_before = state.ll_state.cells_left;

        guess_onto_trail(&mut state, CellIdx(2), Color(1));
        learn_onto_trail(&mut state, CellIdx(3), /*is=*/ true, BACKGROUND);
        state.update_nogood_counters(state.trail.len());

        state.backjump(1);

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

        guess_onto_trail(&mut state, CellIdx(0), Color(1));
        learn_onto_trail(&mut state, CellIdx(2), /*is=*/ true, BACKGROUND);
        guess_onto_trail(&mut state, CellIdx(4), Color(1));
        state.update_nogood_counters(state.trail.len());

        let (nogood, asserting_at) = state.line_is_nogood(LaneIdx(0));
        assert_eq!(
            nogood.not_all_true,
            HashMap::from([
                (CellIdx(0), Cell::from_color(Color(1))),
                (CellIdx(4), Cell::from_color(Color(1)))
            ])
        );
        assert_eq!(nogood.current_false_count, 2);
        assert_eq!(
            asserting_at, 1,
            "rewinding guess 1 leaves cell 0 in place, so the nogood forces cell 4"
        );
    }

    /// `{1,2} . 1` against a clue of `2`: the only `Color(1)` block that fits cell 0 is `0..2`,
    /// which misses cell 2. The lane nogood has to say "cell 0 isn't background", which leaves
    /// it unknown.
    ///
    /// That narrowing came from an older nogood: "not both cell 2 is `Color(1)` and cell 0 is
    /// background". Resolving through it leaves only the guess.
    #[test]
    fn a_lane_nogood_can_lean_on_a_merely_narrowed_cell() {
        let puzzle = one_clue_puzzle(&[Color(1), Color(2)], 3);
        let options = SolveOptions::default();
        let mut line_cache = None;
        let mut ctx = SolveContext::new(&puzzle, &mut line_cache, &options);
        let mut state = scratch_state(
            &puzzle,
            &mut ctx,
            &[(CellIdx(2), Color(1)), (CellIdx(0), BACKGROUND)],
        );

        guess_onto_trail(&mut state, CellIdx(2), Color(1));
        learn_onto_trail_because(
            &mut state,
            CellIdx(0),
            /*is=*/ false,
            BACKGROUND,
            TrailReason::Nogood(0),
        );
        state.update_nogood_counters(state.trail.len());

        let one_or_two = Cell::new(&puzzle.palette).without(Cell::from_color(BACKGROUND));
        assert_eq!(
            state.lane_reason(LaneIdx(0), state.trail.len(), None),
            vec![
                (CellIdx(0), one_or_two),
                (CellIdx(2), Cell::from_color(Color(1)))
            ],
        );

        let (nogood, asserting_at) = state.line_is_nogood(LaneIdx(0));
        assert_eq!(
            nogood.not_all_true,
            HashMap::from([(CellIdx(2), Cell::from_color(Color(1)))])
        );
        assert_eq!(
            asserting_at, 0,
            "back to the root, where it rules out cell 2"
        );
    }

    /// Two rows by three columns, every lane but the last column clued `1`:
    ///
    /// ```text
    ///       1 1 -
    ///    1  a b .
    ///    1  c d g
    /// ```
    ///
    /// Guess `g`. Row 1 then rules out `c` and `d`, the first two columns fill `a` and `b`, and
    /// row 0 can't hold both. Every literal of that contradiction comes from the one guess, so
    /// the bare lane clause ("not both `a` and `b`") doesn't assert. Resolving it walks back
    /// through the columns (to `c`, `d`) and then row 1, until only the guess is left.
    ///
    /// (The trail is built by hand, so the root never runs line logic; the empty column would
    /// otherwise have ruled `g` out before anyone could guess it.)
    #[test]
    fn resolution_walks_a_lane_clause_back_to_the_guess() {
        let mut palette = HashMap::new();
        palette.insert(BACKGROUND, ColorInfo::default_bg());
        palette.insert(Color(1), ColorInfo::default_fg(Color(1)));
        let one = || {
            vec![Nono {
                color: Color(1),
                count: 1,
            }]
        };
        let puzzle = Puzzle::square(palette, vec![one(), one()], vec![one(), one(), vec![]]);

        let options = SolveOptions::default();
        let mut line_cache = None;
        let mut ctx = SolveContext::new(&puzzle, &mut line_cache, &options);
        let (row_0, row_1) = (LaneIdx(0), LaneIdx(1));
        let (col_0, col_1) = (LaneIdx(2), LaneIdx(3));
        let at = |lane: LaneIdx, pos: usize| ctx.lane_map().lanes[lane].cells[LanePos::from(pos)];
        let (a, b, c, d, g) = (
            at(row_0, 0),
            at(row_0, 1),
            at(row_1, 0),
            at(row_1, 1),
            at(row_1, 2),
        );
        let mut state = scratch_state(&puzzle, &mut ctx, &[]);

        guess_onto_trail(&mut state, g, Color(1));
        let mut derive = |cell, color, lane| {
            learn_onto_trail_because(
                &mut state,
                cell,
                /*is=*/ true,
                color,
                TrailReason::Lane(lane),
            )
        };
        derive(c, BACKGROUND, row_1);
        derive(d, BACKGROUND, row_1);
        derive(a, Color(1), col_0);
        derive(b, Color(1), col_1);
        state.update_nogood_counters(state.trail.len());

        assert_eq!(
            state.lane_reason(row_0, state.trail.len(), None),
            vec![
                (a, Cell::from_color(Color(1))),
                (b, Cell::from_color(Color(1)))
            ],
            "the bare clause has two literals at level 1, so it can't assert"
        );

        let (nogood, asserting_at) = state.line_is_nogood(row_0);
        assert_eq!(
            nogood.not_all_true,
            HashMap::from([(g, Cell::from_color(Color(1)))])
        );
        assert_eq!(nogood.current_false_count, 1);
        assert_eq!(asserting_at, 0, "back to the root, where it rules out `g`");
    }

    // End-to-end tests: whole puzzles through `conprop_solve`.

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
        Solution::new(ClueStyle::Nono, bw_palette(), geometry, cells.into())
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

    /// The picture a solved report describes, rendered the way `picture` reads one — so a solved
    /// report can be compared straight against the rows that built the puzzle.
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

    /// How many cells line logic alone leaves unknown, so a test can check it really needs the
    /// search. If line logic gets smarter, these stop testing the search.
    fn line_logic_cells_left<C: Clue, K: GridKind>(puzzle: &Puzzle<C, K>) -> usize {
        let mut grid: PartialSolution =
            vec![Cell::new(&puzzle.palette); puzzle.geometry.cell_count()].into();
        crate::solve::grid_solve::line_logic_solve(
            puzzle,
            &mut None,
            &SolveOptions::default(),
            &mut grid,
        )
        .unwrap()
        .cells_left
    }

    /// A hollow box: line logic alone finishes it, so the search should never start.
    #[test]
    fn a_line_solvable_puzzle_needs_no_search() {
        let want = ["#####", "#...#", "#...#", "#...#", "#####"];
        let puzzle = solution_to_puzzle(&picture(&want));

        let report = conprop_solve(&puzzle, &SolveOptions::default()).unwrap();
        assert_eq!(report.cells_left, 0, "this puzzle has exactly one solution");
        assert_eq!(rendered(&report, 5), want);
    }

    /// Line logic stalls on this one with 18 of its 25 cells unknown. One guess in the
    /// upper-left-hand corner is sufficient to solve it.
    #[test]
    fn a_puzzle_that_needs_a_guess() {
        let want = ["..###", "..#.#", "##...", "....#", ".##.."];
        let puzzle = solution_to_puzzle(&picture(&want));
        assert_eq!(line_logic_cells_left(&puzzle), 18);

        let report = conprop_solve(&puzzle, &SolveOptions::default()).unwrap();
        assert_eq!(report.cells_left, 0, "this puzzle has exactly one solution");
        assert_eq!(rendered(&report, 5), want);
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
        assert_eq!(line_logic_cells_left(&puzzle), 32);

        let report = conprop_solve(&puzzle, &SolveOptions::default()).unwrap();
        assert_eq!(report.cells_left, 0, "this puzzle has exactly one solution");
        assert_eq!(rendered(&report, 7), want);
    }

    /// Three colors, so ruling a cell out doesn't settle it: a nogood's deduction has to leave two
    /// possibilities standing where a black-and-white puzzle would be left with one.
    #[test]
    fn a_multicolor_puzzle_that_needs_a_guess() {
        let want = [".##..", "oo.#.", "o..#o", "o...o", "##..#"];
        let puzzle = solution_to_puzzle(&picture(&want));
        assert_eq!(
            puzzle.palette.len(),
            3,
            "background and two foreground colors"
        );

        let report = conprop_solve(&puzzle, &SolveOptions::default()).unwrap();
        assert_eq!(report.cells_left, 0, "this puzzle has exactly one solution");
        assert_eq!(rendered(&report, 5), want);
    }

    /// Clues with no picture behind them at all — but every lane is satisfiable on its own, and
    /// the row and column totals even agree, so line logic runs out of things to say with 16
    /// cells still unknown rather than reporting a contradiction. Only the search can find out.
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
        assert_eq!(line_logic_cells_left(&puzzle), 16);

        assert!(conprop_solve(&puzzle, &SolveOptions::default()).is_err());
    }

    /// The search is generic over the grid shape, and a triddler is the part of that generality a
    /// square puzzle can't reach: three clue directions instead of two, and lanes of differing
    /// lengths that meet in places no row-and-column shortcut would predict. Line logic gets 6 of
    /// these 16 cells and stops.
    #[test]
    fn a_triddler_that_needs_a_guess() {
        let want = [".#..#", "..##..", "....."];
        let puzzle = solution_to_tri_puzzle(&tri_picture(&want));
        assert_eq!(line_logic_cells_left(&puzzle), 10);

        let report = conprop_solve(&puzzle, &SolveOptions::default()).unwrap();
        assert_eq!(report.cells_left, 0, "this puzzle has exactly one solution");
        // The lanes are ragged, so `rendered`'s fixed-width rows don't apply; compare against the
        // picture the clues came from instead.
        assert_eq!(report.solution.cells(), tri_picture(&want).cells);
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

        assert!(conprop_solve(&puzzle, &SolveOptions::default()).is_err());
    }

    /// One filled cell per row and per column of a 2x2 grid: the two diagonals both fit.
    #[test]
    fn an_ambiguous_puzzle_reports_multiple_solutions() {
        let puzzle = solution_to_puzzle(&picture(&["#.", ".#"]));

        let report = conprop_solve(&puzzle, &SolveOptions::default()).unwrap();
        assert!(report.cells_left > 0, "both diagonals fit these clues");
    }

    /// A run that doesn't fit in the lane it's a clue for. Line logic sees the contradiction on
    /// its first pass, before the search ever starts, so it has to arrive as an error rather than
    /// as a report of a puzzle with no solutions.
    #[test]
    fn impossible_clues_are_an_error() {
        // Two columns, so the first row's run of three has nowhere to go.
        let puzzle = Puzzle::square(
            bw_palette(),
            vec![runs(&[3]), runs(&[1])],
            vec![runs(&[1]), runs(&[1])],
        );

        assert!(conprop_solve(&puzzle, &SolveOptions::default()).is_err());
    }

    /// The GUI's way in: same answer, delivered through the async wrapper, with the progress
    /// channel finishing at 1.0.
    #[test]
    fn the_background_solve_finishes_with_full_progress() {
        let want = [
            "...##..", ".#.#...", "##..##.", "..##.##", "##.....", "#..#..#", ".##.#.#",
        ];
        let puzzle = solution_to_puzzle(&picture(&want));

        let (progress_s, progress_r) = mpsc::channel();
        let report = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(conprop_solve_in_background(
                &puzzle,
                &SolveOptions::default(),
                progress_s,
                mpsc::channel().1,
            ))
            .unwrap();
        assert_eq!(rendered(&report, 7), want);

        let reported: Vec<f32> = progress_r.try_iter().collect();
        assert!(reported.iter().all(|p| *p > 0.0), "{reported:?}");
        assert_eq!(reported.last(), Some(&1.0));
    }
}
