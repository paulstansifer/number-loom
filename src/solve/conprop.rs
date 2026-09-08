use std::collections::HashSet;

use anyhow::bail;

use crate::{
    geometry::GridKind,
    puzzle::{Clue, Color, PartialSolution, Puzzle},
    solve::{
        conprop_picking::pick_guess,
        grid_solve::{LineCache, Report, SolveContext, SolveOptions, SolveState},
        line_solve::Cell,
    },
};

struct Nogood {
    not_all_true: HashSet<(usize, Color)>,
    current_false_count: usize,
    active: bool,
}

impl Nogood {
    /// An error if the nogood is already contradicted, `Ok(Some(cell, color))` if `cell` can't be `color`
    fn deduction<C: Clue>(
        &self,
        ll_state: &SolveState<C>,
    ) -> anyhow::Result<Option<(usize, Color)>> {
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
            if !ll_state.grid[cell_idx].is_known_to_be(color) {
                return Ok(Some((cell_idx, color)));
            }
        }
        bail!("Nogood contradicted, and `current_false_count` was stale");
    }

    fn inform(&mut self, cell_idx: usize, color: Color, forwards: bool) {
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

// TODO: there are a bunch of indices

struct ConpropState<'p, C: Clue> {
    nogoods: Vec<Nogood>,
    // Outer is indexable by `cell_idx`, inner contains indices to `nogoods`
    nogoods_by_cell: Vec<Vec<usize>>,
    trail: Vec<(usize, Cell)>, // (cell_idx, old_value)

    nogoods_upated_to: usize, // index into trail: where are the nogoods current up to?
    guesses_in_trail: Vec<(usize, Color)>, // (index into trail, guessed color)
    ll_state: SolveState<'p, C>,

    guesses_made: usize,
    // TODO: feed this to the picker, so it only picks things that contradict this
    solution_found: Option<PartialSolution>, // never actually "Partial", of course.
    /// What the puzzle pins down on its own, and how many cells that leaves unknown — taken at
    /// the root the moment `solution_found` was set, and never touched again. Set exactly when
    /// `solution_found` is; this is what an ambiguous puzzle gets reported as.
    root_knowledge: Option<(PartialSolution, usize)>,
}

impl<'p, C: Clue> ConpropState<'p, C> {
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
        // Claude: please test for off-by-one errors in the backjump.
        let (trail_idx, _guess) = self.guesses_in_trail[guess_idx];
        self.update_nogood_counters(trail_idx);
        self.ll_state.unwind(linear_ctx, &self.trail[trail_idx..]);

        self.trail.truncate(trail_idx);
        self.guesses_in_trail.truncate(guess_idx);
    }

    /// Start watching a nogood.
    fn add_nogood(&mut self, nogood: Nogood) {
        let nogood_idx = self.nogoods.len();
        for &(cell_idx, _) in &nogood.not_all_true {
            self.nogoods_by_cell[cell_idx].push(nogood_idx);
        }
        self.nogoods.push(nogood);
    }

    /// "Not all of the guesses currently on the trail" — the nogood that rules out the solution
    /// they led to, and nothing else, so the search can go looking for a second one.
    ///
    /// It's sound because propagation is: a complete grid that agreed with every one of these
    /// guesses would agree with everything they imply, which is this whole grid. So any *other*
    /// solution has to disagree with at least one guess on this list.
    fn solution_nogood(&self) -> Nogood {
        let not_all_true: HashSet<(usize, Color)> = self
            .guesses_in_trail
            .iter()
            .map(|&(trail_idx, color)| (self.trail[trail_idx].0, color))
            .collect();

        Nogood {
            current_false_count: not_all_true.len(), // every one of them holds right now
            not_all_true,
            active: false,
        }
    }

    /// The `Nogood`s are out-of-date, adjust them.
    fn update_nogood_counters(&mut self, new_idx: usize) {
        let forwards = new_idx > self.nogoods_upated_to;
        let range = if forwards {
            self.nogoods_upated_to..new_idx
        } else {
            new_idx..self.nogoods_upated_to
        };

        // TODO: test that there are no off-by-one errors here

        let mut cells_seen = HashSet::<usize>::new();
        for &(cell_idx, _) in &self.trail[range] {
            if !cells_seen.insert(cell_idx) {
                continue; // don't double-count! All we're doing here is seeing whether the cell became known
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

        self.nogoods_upated_to = new_idx;
    }

    /// Applies linear logic and nogoods until everything possible is deduced. `Ok(true)` if a solution is found.
    fn propagate<'x, K: GridKind>(
        &mut self,
        linear_ctx: &mut SolveContext<'p, 'x, C, K>,
    ) -> anyhow::Result<bool> {
        let linear_res = self
            .ll_state
            .run_and_check_recording(linear_ctx, &mut self.trail);
        self.update_nogood_counters(self.trail.len());

        linear_res?; // Had to update the counters first.

        let mut any_nogoods_fired = false;

        for nogood in &mut self.nogoods {
            if let Some((cell_idx, is_not_color)) = nogood.deduction(&self.ll_state)? {
                any_nogoods_fired = true;
                nogood.active = true;

                let before = self.ll_state.grid[cell_idx];
                if self
                    .ll_state
                    .learn(linear_ctx, cell_idx, /*is=*/ false, is_not_color)?
                {
                    // Only when it actually moved: a no-op entry is a cell the trail claims
                    // changed when it didn't, and `update_nogood_counters` believes the trail.
                    self.trail.push((cell_idx, before));
                }
                // TODO: borrow checker fights against updating the counters here
                // ...but that just means the next recursion picks them up. Right, Claude?
            }
        }

        if any_nogoods_fired {
            self.propagate(linear_ctx) // Go around again, see if there's more to do!
        } else {
            Ok(self.ll_state.cells_left == 0)
        }
    }

    fn propagate_and_learn<'x, K: GridKind>(
        &mut self,
        puzzle: &Puzzle<C, K>,
        linear_ctx: &mut SolveContext<'p, 'x, C, K>,
    ) -> Option<Report> {
        let state = self;
        let mut run_res = state.propagate(linear_ctx);
        while run_res.is_err() {
            if state.guesses_in_trail.is_empty() {
                // Nothing hypothetical is left to blame it on: the nogoods have made the root
                // itself contradictory, so there is nothing further out there to find. (This is
                // also what keeps `make_nogood` from ever being asked for an empty nogood.)
                return Some(state.no_guesses_left(puzzle));
            }
            let (nogood, backjump_guess_idx) = state.make_nogood(puzzle, linear_ctx);
            state.add_nogood(nogood);
            state.backjump(backjump_guess_idx, linear_ctx);

            run_res = state.propagate(linear_ctx)
        }
        if run_res.unwrap() {
            // We found a valid solution!
            if let Some(existing_solution) = &state.solution_found {
                assert!(
                    *existing_solution != state.ll_state.grid,
                    "TODO: I thought we couldn't reach the same solution multiple times"
                );

                // Puzzle has multiple valid solutions. Report what holds unconditionally, from
                // before the first solution's nogood started ruling real grids out. Rewinding to
                // the root *now* wouldn't do: the grid down there has that nogood's consequences
                // baked into it, and this very branch is the proof that they aren't the puzzle's.
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
                // Only ever asked for *a* solution, so stop before spending the rest of the
                // search proving there isn't a second one. `cells_left` of 0 means "a complete
                // grid" here rather than "the only grid" -- nothing went looking for another.
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

            // Move the goalposts: now try to find a second solution. Build and register the
            // nogood *before* rewinding, so that `update_nogood_counters` hears about the guesses
            // coming off — it only tells nogoods it can already see.
            let first_solution_is_nogood = state.solution_nogood();
            state.add_nogood(first_solution_is_nogood);

            // Then unwind every guess and look at what's left. That grid is what line logic and
            // the conflict nogoods settled with nothing assumed — exactly what the puzzle pins
            // down by itself — and it's the last honest look we get at it: from here on the
            // nogood above is in play, and it rules out a grid that really does fit the clues, so
            // anything downstream of it is only true of solutions *other* than this one.
            // Registering a nogood doesn't move any cells, so the picture is still clean.
            state.backjump(0, linear_ctx);
            state.root_knowledge = Some((state.ll_state.grid.clone(), state.ll_state.cells_left));

            return state.propagate_and_learn(puzzle, linear_ctx);
        } else {
            return None; // No solution, no error.
        }
    }

    /// Minimizes the tail of the `Nogood`, and returns an index (to `guesses_in_trail`) to backjump past,
    /// or `None` if all the guesses should be cleared.
    fn make_nogood<'x, K: GridKind>(
        &self,
        puzzle: &Puzzle<C, K>,
        linear_ctx: &mut SolveContext<'p, 'x, C, K>,
    ) -> (Nogood, usize) {
        let mut linear_state = SolveState::new(
            linear_ctx,
            vec![Cell::new(&puzzle.palette); puzzle.geometry.cell_count()],
        );
        let mut res = Nogood {
            not_all_true: HashSet::new(),
            current_false_count: 0,
            active: false,
        };

        let Some((&(bad_trail_idx, bad_color), guesses_pfx)) = self.guesses_in_trail.split_last()
        else {
            return (res, 0); // No guesses, so the puzzle is contradictory.
        };
        let bad_cell = self.trail[bad_trail_idx].0;

        res.not_all_true.insert((bad_cell, bad_color));
        res.current_false_count = 1;

        if guesses_pfx.is_empty() {
            return (res, 0); // This was the only guess; nothing to minimize!
        }

        // Start with the guess that went wrong
        linear_state.learn_new(linear_ctx, bad_cell, /*is*/ true, bad_color);
        // TODO: use the nogoods here, otherwise we might not find a contradiction at all!
        if linear_state.run_and_check(linear_ctx).is_err() {
            return (res, 0); // This guess is wrong on its own!
        }

        // TODO: try binary search, like the big kids
        let mut penultimate_critical_guess = None;
        // Reapply other guesses:
        for (guess_idx, &(trail_idx, color)) in guesses_pfx.iter().enumerate() {
            let cell = self.trail[trail_idx].0;

            // Reapply guess and see if it's okay:
            if linear_state
                .learn(linear_ctx, cell, /*is*/ true, color)
                .is_err()
                || linear_state.run_and_check(linear_ctx).is_err()
            {
                penultimate_critical_guess = Some(guess_idx);
                break;
            }
        }
        // TODO: also try peeling things off the front.

        // Fallback if we didn't find a contradiction: use everything.
        // TODO: log how many times we have to be conservative.
        // Claude, please check for an off-by-one error here:
        let penultimate_critical_guess =
            penultimate_critical_guess.unwrap_or(guesses_pfx.len() - 1);

        for &(trail_idx, color) in &self.guesses_in_trail[0..=penultimate_critical_guess] {
            let cell = self.trail[trail_idx].0;
            res.not_all_true.insert((cell, color));
        }
        res.current_false_count = res.not_all_true.len(); // currently *at* a contradiction!

        // We want to *keep* the penultimate critical guess:
        (res, penultimate_critical_guess + 1)
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
        vec![Cell::new(&puzzle.palette); puzzle.geometry.cell_count()],
    );
    linear_state.run_and_check(&mut linear_ctx)?; // `?` because contradictions here are "real"

    if linear_state.cells_left == 0 {
        // let _ = ctx.progress.send(1.0);
        return Ok(linear_state.report(puzzle)); // No fancy stuff required!
    }

    let mut state = ConpropState {
        nogoods: vec![],
        nogoods_by_cell: vec![vec![]; linear_state.grid.len()],
        trail: vec![],
        nogoods_upated_to: 0,
        guesses_in_trail: vec![],
        ll_state: linear_state,
        guesses_made: 0,
        solution_found: None,
        root_knowledge: None,
    };

    loop {
        let picker = linear_ctx
            .options
            .guess_picker_conprop
            .for_guess(state.guesses_made);

        let Some((cell_idx, color)) = pick_guess(picker, &state.ll_state, &linear_ctx) else {
            assert!(state.guesses_in_trail.is_empty());
            return Ok(state.no_guesses_left(puzzle));
        };

        state.guesses_made += 1;

        state.guesses_in_trail.push((state.trail.len(), color));
        // TODO: fold the `trail` update in `.learn` ... if this wins out over `bt_solve`
        state.trail.push((cell_idx, state.ll_state.grid[cell_idx]));
        state
            .ll_state
            .learn_new(&mut linear_ctx, cell_idx, /*is*/ true, color);

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
        literals: &[(usize, Color)],
    ) -> ConpropState<'p, Nono> {
        let cell_count = puzzle.geometry.cell_count();
        let mut nogoods_by_cell = vec![vec![]; cell_count];
        for &(cell, _) in literals {
            nogoods_by_cell[cell].push(0);
        }

        ConpropState {
            nogoods: vec![Nogood {
                not_all_true: literals.iter().copied().collect(),
                current_false_count: 0,
                active: false,
            }],
            nogoods_by_cell,
            trail: vec![],
            nogoods_upated_to: 0,
            guesses_in_trail: vec![],
            ll_state: SolveState::new(ctx, vec![Cell::new(&puzzle.palette); cell_count]),
            guesses_made: 0,
            solution_found: None,
            root_knowledge: None,
        }
    }

    /// Learn one fact, recording it on the trail the way the search does.
    fn learn_onto_trail<'p>(
        state: &mut ConpropState<'p, Nono>,
        ctx: &mut SolveContext<'p, '_, Nono, Square>,
        cell: usize,
        is: bool,
        color: Color,
    ) {
        state.trail.push((cell, state.ll_state.grid[cell]));
        assert!(
            state.ll_state.learn(ctx, cell, is, color).unwrap(),
            "the test meant to learn something new about cell {cell}"
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
        let literals = [(0, Color(1)), (1, Color(1)), (3, BACKGROUND)];
        let mut state = scratch_state(&puzzle, &mut ctx, &literals);

        // One trail entry each, in an order that puts a cell the nogood says nothing about
        // (cell 2) in the middle, so a range that runs one too far can't hide behind a hit.
        for (cell, color) in [(0, Color(1)), (2, Color(2)), (1, Color(1)), (3, BACKGROUND)] {
            learn_onto_trail(&mut state, &mut ctx, cell, /*is=*/ true, color);
        }

        // Nothing folded in yet: the counter describes a grid where none of this has happened.
        assert_eq!(state.nogoods[0].current_false_count, 0);

        // An empty range changes nothing.
        state.update_nogood_counters(0);
        assert_eq!(state.nogoods[0].current_false_count, 0);
        assert_eq!(state.nogoods_upated_to, 0);

        // `1` takes in entry 0 — cell 0, which the nogood names — and stops before entry 1.
        state.update_nogood_counters(1);
        assert_eq!(
            state.nogoods[0].current_false_count, 1,
            "one entry in, only cell 0 should have counted"
        );
        assert_eq!(state.nogoods_upated_to, 1);

        // Entry 1 is cell 2, which the nogood doesn't name.
        state.update_nogood_counters(2);
        assert_eq!(state.nogoods[0].current_false_count, 1);

        // Entries 2 and 3 are the other two literals, taken in one jump.
        state.update_nogood_counters(4);
        assert_eq!(state.nogoods[0].current_false_count, 3);
        assert_eq!(state.nogoods_upated_to, 4);

        // Rewinding walks the same half-open range the other way. It reads the grid as it stands,
        // so it has to run *before* `SolveState::unwind` puts the cells back.
        state.update_nogood_counters(2);
        assert_eq!(
            state.nogoods[0].current_false_count, 1,
            "rewinding to 2 should undo entries 2 and 3, and no more"
        );
        assert_eq!(state.nogoods_upated_to, 2);

        state.update_nogood_counters(0);
        assert_eq!(state.nogoods[0].current_false_count, 0);
        assert_eq!(state.nogoods_upated_to, 0);
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

        let literals = [(0, Color(1))];
        let mut state = scratch_state(&puzzle, &mut ctx, &literals);

        // {bg,1,2} minus 2 is still unknown, so this entry settles nothing and counts for nothing.
        learn_onto_trail(&mut state, &mut ctx, 0, /*is=*/ false, Color(2));
        assert!(!state.ll_state.grid[0].is_known());
        state.update_nogood_counters(1);
        assert_eq!(
            state.nogoods[0].current_false_count, 0,
            "cell 0 wasn't known yet after the first narrowing"
        );

        // The second entry is where the cell lands on a color.
        learn_onto_trail(&mut state, &mut ctx, 0, /*is=*/ true, Color(1));
        assert!(state.ll_state.grid[0].is_known_to_be(Color(1)));
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
        cell: usize,
        color: Color,
    ) {
        state.guesses_in_trail.push((state.trail.len(), color));
        learn_onto_trail(state, ctx, cell, /*is=*/ true, color);
    }

    /// A trail of two guesses, each followed by one consequence, so the guesses sit at trail
    /// entries 0 and 2 and there is an entry on either side of every boundary worth probing.
    fn two_guesses_deep<'p>(
        state: &mut ConpropState<'p, Nono>,
        ctx: &mut SolveContext<'p, '_, Nono, Square>,
    ) {
        guess_onto_trail(state, ctx, 0, Color(1));
        learn_onto_trail(state, ctx, 1, /*is=*/ true, Color(2));
        guess_onto_trail(state, ctx, 2, Color(1));
        learn_onto_trail(state, ctx, 3, /*is=*/ true, BACKGROUND);
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
        let literals = [(0, Color(1)), (3, BACKGROUND)];
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
        assert_eq!(state.nogoods_upated_to, 2);
        assert_eq!(
            state.nogoods[0].current_false_count, 1,
            "cell 3 was rewound past and shouldn't still count; cell 0 wasn't and should"
        );

        // Just before guess 0 is the root: nothing survives.
        state.backjump(0, &mut ctx);
        assert!(state.trail.is_empty());
        assert!(state.guesses_in_trail.is_empty());
        assert_eq!(state.nogoods_upated_to, 0);
        assert_eq!(state.nogoods[0].current_false_count, 0);
    }

    /// You're right that a nogood naming two or more cells is fine: it can only have fired once
    /// the other literals were satisfied, and it fires in the same `propagate` that made it unit,
    /// so its deduction lands in the same guess block. Any rewind that reaches the deduction has
    /// to reach that literal too, and `inform` clears the flag on the way past.
    ///
    /// A nogood naming *one* cell has no other literal to lean on — and `make_nogood` returns one
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

        // "cell 0 is not Color(1)" — what `make_nogood` hands back for a guess that's wrong alone.
        let literals = [(0, Color(1))];
        let mut state = scratch_state(&puzzle, &mut ctx, &literals);

        // A guess about something else, so there's a block for the deduction to live in.
        guess_onto_trail(&mut state, &mut ctx, 2, Color(2));

        // Fire the nogood the way `propagate` does.
        assert_eq!(
            state.nogoods[0].deduction(&state.ll_state).unwrap(),
            Some((0, Color(1))),
            "a one-cell nogood with nothing counted against it is unit"
        );
        state.nogoods[0].active = true;
        state.trail.push((0, state.ll_state.grid[0]));
        state
            .ll_state
            .learn(&mut ctx, 0, /*is=*/ false, Color(1))
            .unwrap();
        assert!(!state.ll_state.grid[0].is_known(), "still {{bg, 2}}");

        // One more entry after it, so the deduction isn't the last thing on the trail.
        learn_onto_trail(&mut state, &mut ctx, 3, /*is=*/ true, BACKGROUND);
        state.update_nogood_counters(state.trail.len());

        state.backjump(0, &mut ctx);

        assert!(
            state.ll_state.grid[0].can_be(Color(1)),
            "the deduction was rewound, so cell 0 can be Color(1) again"
        );
        assert!(
            !state.nogoods[0].active,
            "the deduction is gone, so the nogood has to be willing to make it again"
        );
        assert_eq!(
            state.nogoods[0].deduction(&state.ll_state).unwrap(),
            Some((0, Color(1)))
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

        let literals = [(0, Color(1)), (3, BACKGROUND)];
        let mut state = scratch_state(&puzzle, &mut ctx, &literals);

        guess_onto_trail(&mut state, &mut ctx, 0, Color(1));
        learn_onto_trail(&mut state, &mut ctx, 1, /*is=*/ true, Color(2));

        // What the second guess is about to be made from.
        let grid_before = state.ll_state.grid.clone();
        let cells_left_before = state.ll_state.cells_left;

        guess_onto_trail(&mut state, &mut ctx, 2, Color(1));
        learn_onto_trail(&mut state, &mut ctx, 3, /*is=*/ true, BACKGROUND);
        state.update_nogood_counters(state.trail.len());

        state.backjump(1, &mut ctx);

        assert_eq!(
            state.ll_state.grid, grid_before,
            "the cells guess 1 settled are still settled after backjumping past it"
        );
        assert_eq!(state.ll_state.cells_left, cells_left_before);
    }
}
