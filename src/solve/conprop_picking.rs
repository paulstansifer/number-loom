//! Deciding *where* to guess, once line logic has stalled and `conprop` has to branch.
//!
//! A `GuessPicker` rates the guesses available in the current state and names the best one;
//! `PickerMix` says which picker each guess gets, so a search can rotate through several. This
//! is the part of the search worth experimenting with, so it lives apart from the search itself,
//! which doesn't care how the guesses get chosen.
//!
//! Unlike `bt_solve`, `conprop` has no tree of nodes: there is one line-logic state, guesses are
//! pushed onto it and unwound off it, and a guess that gets backjumped out of is ruled out by
//! the nogood the backjump learned rather than by a per-node "already tried" set. So everything
//! here reads a plain `SolveState`, and `pick` considers every color every unknown cell can
//! still be.

use crate::{
    geometry::GridKind,
    puzzle::{BACKGROUND, Clue, Color},
    solve::grid_solve::{SolveContext, SolveState},
};

/// Decides which cell to guess at, and what color to guess.
///
/// `rate` runs once per candidate guess — that's every unknown cell times every color it could
/// be — so anything that's the same for all of the state's guesses belongs in `new`, which runs
/// once per guess, just before the candidates it's about to rate.
trait GuessPicker: Sized {
    /// Precompute whatever `rate` shouldn't be doing over and over. `state` is the grid whose
    /// guesses this picker will rate; a picker that only cares about the puzzle's shape can
    /// ignore it.
    fn new<C: Clue, K: GridKind>(
        state: &SolveState<'_, C>,
        linear_ctx: &SolveContext<'_, '_, C, K>,
    ) -> Self;

    /// Score guesses against each other.
    fn rate<C: Clue, K: GridKind>(
        &self,
        state: &SolveState<'_, C>,
        linear_ctx: &SolveContext<'_, '_, C, K>,
        guess: (usize, Color),
    ) -> Score;

    /// Pick the lowest-scoring choice that's a valid guess
    fn pick<C: Clue, K: GridKind>(
        &self,
        state: &SolveState<'_, C>,
        linear_ctx: &SolveContext<'_, '_, C, K>,
    ) -> Option<(usize, Color)> {
        let idxed_cells = state.grid.iter().enumerate();
        let uncertain_cells = idxed_cells.filter(|(_, cell)| !cell.is_known());
        let options = uncertain_cells
            .flat_map(|(idx, cell)| cell.can_be_iter().map(move |color| (idx, color)));
        // `min_by_key` keeps the first of a tie, like the stable sort this used to do.
        options.min_by_key(|guess| self.rate(state, linear_ctx, *guess))
    }
}

/// Lower is better! Only used to rank the guesses available at one point in the search against
/// each other; the numbers mean nothing between pickers.
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

struct First;

impl GuessPicker for First {
    fn new<C: Clue, K: GridKind>(_: &SolveState<'_, C>, _: &SolveContext<'_, '_, C, K>) -> First {
        First
    }

    fn rate<C: Clue, K: GridKind>(
        &self,
        _: &SolveState<'_, C>,
        _: &SolveContext<'_, '_, C, K>,
        _: (usize, Color),
    ) -> Score {
        Score(0.0)
    }
}

/// Well, this one seems better, but performs worse.
struct Edge {
    /// Overall score for closeness to edge, by cell index.
    edginess: Vec<f32>,
}

impl GuessPicker for Edge {
    fn new<C: Clue, K: GridKind>(
        _: &SolveState<'_, C>,
        linear_ctx: &SolveContext<'_, '_, C, K>,
    ) -> Edge {
        let lane_map = linear_ctx.lane_map();

        let mut dists: Vec<usize> = vec![];
        let edginess = (0..lane_map.cell_count() as u32)
            .map(|cell| {
                dists.clear();
                dists.extend(lane_map.memberships(cell).iter().map(|m| {
                    let len = lane_map.lane(m.lane as usize).cells.len();
                    let pos = m.position as usize;
                    pos.min(len - (pos + 1))
                }));
                dists.sort();
                dists[0] as f32 + dists[1] as f32 * 0.1
            })
            .collect();

        Edge { edginess }
    }

    fn rate<C: Clue, K: GridKind>(
        &self,
        _: &SolveState<'_, C>,
        _: &SolveContext<'_, '_, C, K>,
        (idx, _): (usize, Color),
    ) -> Score {
        Score(self.edginess[idx])
    }
}

/// Inverse of `Edge`; surprisingly, it seems to be slightly better (on limited and outdated testing)
struct Middle(Edge);

impl GuessPicker for Middle {
    fn new<C: Clue, K: GridKind>(
        state: &SolveState<'_, C>,
        linear_ctx: &SolveContext<'_, '_, C, K>,
    ) -> Middle {
        Middle(Edge::new(state, linear_ctx))
    }

    fn rate<C: Clue, K: GridKind>(
        &self,
        state: &SolveState<'_, C>,
        linear_ctx: &SolveContext<'_, '_, C, K>,
        guess: (usize, Color),
    ) -> Score {
        Score(-self.0.rate(state, linear_ctx, guess).0)
    }
}

/// A guess picked uniformly at random — the baseline any heuristic ought to beat.
struct Random {
    /// `rate` takes `&self`, so the stream needs interior mutability.
    rng: std::cell::Cell<u64>,
}

impl GuessPicker for Random {
    fn new<C: Clue, K: GridKind>(
        state: &SolveState<'_, C>,
        _: &SolveContext<'_, '_, C, K>,
    ) -> Random {
        use std::hash::BuildHasher;

        // `RandomState`'s keys differ from one instance to the next, so this is a fresh stream
        // per guess, and a different search every run.
        let seed = std::collections::hash_map::RandomState::new().hash_one(state.cells_left);

        Random {
            rng: std::cell::Cell::new(seed | 1), // xorshift never leaves zero
        }
    }

    fn rate<C: Clue, K: GridKind>(
        &self,
        _: &SolveState<'_, C>,
        _: &SolveContext<'_, '_, C, K>,
        _: (usize, Color),
    ) -> Score {
        let mut x = self.rng.get(); // xorshift64
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng.set(x);

        Score((x >> 40) as f32) // the high bits are the well-mixed ones
    }
}

/// Guesses where two lanes disagree about what a cell probably is.
///
/// One lane on its own gives a naïve probability for each color based on the clue colors minus
/// the existing known cells; compare those probabilities between rows and columns and look for
/// surprises.
struct Disagreement {
    /// Indexed by `cell * stride + color.0`: how good a guess that cell/color pair is.
    ratings: Vec<f32>,
    stride: usize,
}

impl GuessPicker for Disagreement {
    fn new<C: Clue, K: GridKind>(
        state: &SolveState<'_, C>,
        linear_ctx: &SolveContext<'_, '_, C, K>,
    ) -> Disagreement {
        let lane_map = linear_ctx.lane_map();
        let puzzle = linear_ctx.puzzle;
        let stride = puzzle
            .palette
            .keys()
            .map(|c| c.0 as usize)
            .max()
            .unwrap_or(0)
            + 1;

        // Per lane, the naïve probability of each color.
        let mut lane_p = vec![0.0_f32; lane_map.lane_count() * stride];
        let mut wanted = vec![0_i32; stride];
        for (lane_idx, lane) in lane_map.lanes().iter().enumerate() {
            wanted.fill(0);

            // What the clues call for...
            for clue in &puzzle.lines[lane_idx] {
                for (color, range) in clue.color_ranges() {
                    wanted[color.0 as usize] += range.len() as i32;
                }
            }
            // ...everything the clues don't mention is background...
            let foreground: i32 = wanted.iter().sum();
            wanted[BACKGROUND.0 as usize] += lane.cells.len() as i32 - foreground;

            // ...minus what's already on the grid.
            let mut unknown = 0;
            for cell_idx in &lane.cells {
                match state.grid[*cell_idx as usize].known_or() {
                    Some(color) => wanted[color.0 as usize] -= 1,
                    None => unknown += 1,
                }
            }

            for color in 0..stride {
                lane_p[lane_idx * stride + color] = if unknown == 0 {
                    0.0
                } else {
                    wanted[color].max(0) as f32 / unknown as f32
                };
            }
        }

        // Now rate each cell by how much the lanes crossing it disagree.
        let mut ratings = vec![0.0_f32; lane_map.cell_count() * stride];
        for cell in 0..lane_map.cell_count() as u32 {
            let memberships = lane_map.memberships(cell);
            for color in 0..stride {
                let mut lowest = f32::MAX;
                let mut highest = f32::MIN;
                let mut total = 0.0;
                for m in memberships {
                    let p = lane_p[m.lane as usize * stride + color];
                    lowest = lowest.min(p);
                    highest = highest.max(p);
                    total += p;
                }
                let spread = highest - lowest;
                let mean = total / memberships.len() as f32;

                // Disagreement decides *where* to guess; within a cell, the likelier color goes
                // first, which only ever breaks a tie (both terms are in `0.0..=1.0`).
                ratings[cell as usize * stride + color] = -spread - 0.1 * mean;
            }
        }

        Disagreement { ratings, stride }
    }

    fn rate<C: Clue, K: GridKind>(
        &self,
        _: &SolveState<'_, C>,
        _: &SolveContext<'_, '_, C, K>,
        (idx, color): (usize, Color),
    ) -> Score {
        Score(self.ratings[idx * self.stride + color.0 as usize])
    }
}

/// How many of each cell's neighbors `counts` accepts. A neighbor is the cell before or after
/// this one in a lane it belongs to — on a square grid, exactly the four cells sharing an edge;
/// on a triddler, the six cells line logic can reach it through. A lane that ends here has no
/// neighbor on that side, and contributes `edge` instead.
fn neighborhood<C: Clue, K: GridKind>(
    linear_ctx: &SolveContext<'_, '_, C, K>,
    edge: f32,
    counts: impl Fn(usize) -> bool,
) -> Vec<f32> {
    let lane_map = linear_ctx.lane_map();

    (0..lane_map.cell_count() as u32)
        .map(|cell| {
            let mut total = 0.0;
            for m in lane_map.memberships(cell) {
                let lane = &lane_map.lane(m.lane as usize).cells;
                let pos = m.position as usize;

                let before = pos.checked_sub(1).map(|p| lane[p]);
                let after = lane.get(pos + 1).copied();
                for side in [before, after] {
                    total += match side {
                        Some(neighbor) if counts(neighbor as usize) => 1.0,
                        Some(_) => 0.0,
                        None => edge,
                    };
                }
            }
            total
        })
        .collect()
}

/// Guesses where the most is already settled: a cell whose neighbors are known is one whose
/// color the lines around it have the most to say about, so a guess there is the likeliest to
/// propagate (or to be contradicted quickly, which is just as useful).
///
/// The puzzle's edge counts as slightly *more* than a solved neighbor, so among cells that are
/// equally hemmed in, the ones against a wall go first.
struct Neighbors {
    /// How settled each cell's surroundings are, by cell index.
    solidity: Vec<f32>,
}

impl GuessPicker for Neighbors {
    fn new<C: Clue, K: GridKind>(
        state: &SolveState<'_, C>,
        linear_ctx: &SolveContext<'_, '_, C, K>,
    ) -> Neighbors {
        let solidity = neighborhood(
            linear_ctx,
            /*edge=*/ 1.01,
            |neighbor| state.grid[neighbor].is_known(),
        );

        Neighbors { solidity }
    }

    fn rate<C: Clue, K: GridKind>(
        &self,
        _: &SolveState<'_, C>,
        _: &SolveContext<'_, '_, C, K>,
        (idx, _): (usize, Color),
    ) -> Score {
        Score(-self.solidity[idx]) // more settled is better, and lower is better
    }
}

/// Which `GuessPicker` `conprop_solve` uses. The search's shape depends entirely on where it
/// decides to guess, so this is the knob worth benchmarking.
#[derive(clap::ValueEnum, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum PickerKind {
    /// The first unknown cell, in grid order.
    First,
    /// Nearest the end of a lane.
    Edge,
    /// Furthest from the end of a lane.
    Middle,
    /// Uniformly at random.
    Random,
    /// Where the lanes crossing a cell disagree most about its color. The best of the lot on
    /// the `examples/wolter` set: the only one that solves `webpbn-00803` at all.
    #[default]
    Disagreement,
    /// Where the most neighboring cells are already solved (the puzzle's edge counting for a
    /// little more than one of them).
    Neighbors,
}

impl PickerKind {
    pub const ALL: [PickerKind; 6] = [
        PickerKind::First,
        PickerKind::Edge,
        PickerKind::Middle,
        PickerKind::Random,
        PickerKind::Disagreement,
        PickerKind::Neighbors,
    ];

    /// The name `--picker` spells this kind with. `picker_names_round_trip` checks these
    /// against what the `ValueEnum` derive accepts, so the two can't drift apart.
    fn name(&self) -> &'static str {
        match self {
            PickerKind::First => "first",
            PickerKind::Edge => "edge",
            PickerKind::Middle => "middle",
            PickerKind::Random => "random",
            PickerKind::Disagreement => "disagreement",
            PickerKind::Neighbors => "neighbors",
        }
    }
}

/// How many pickers one rotation may hold, so that a fat-fingered weight can't ask for a
/// gigabyte of schedule.
const MAX_ROTATION: usize = 1024;

/// The pickers a search rotates through, and in what proportion — `disagreement:3,random:1`
/// takes three guesses with `Disagreement` and then one at random, over and over.
///
/// Pickers turn out to be complementary rather than ranked: on the `examples/wolter` set,
/// `Disagreement` is the only one that cracks `webpbn-00803` and the only one that doesn't crack
/// `webpbn-06574`. A mix is a way to get both without knowing in advance which puzzle you have.
/// Diluting a good pair does hurt, though: adding `middle:1,first:1` to the default's rotation
/// costs three puzzles and gains none, since every guess spent on a picker that isn't steering
/// is one the two that are don't get.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickerMix {
    /// The rotation, with the weights already spelled out: `disagreement:3,random:1` is stored
    /// as three `Disagreement`s and a `Random`, and `for_guess` just indexes it.
    rotation: Vec<PickerKind>,
}

impl Default for PickerMix {
    /// `Disagreement` and `Neighbors`, alternating — the pair `bt_solve` settled on. Each does
    /// something the other can't: only `Disagreement` gets `webpbn-00803`, only `Neighbors` gets
    /// `webpbn-03541`. Having no `Random` in it, it also makes the same choices every run, so a
    /// future change to the solver shows up as a difference rather than as noise.
    fn default() -> PickerMix {
        use PickerKind::{Disagreement, Neighbors};
        PickerMix {
            rotation: vec![Disagreement, Neighbors],
        }
    }
}

impl PickerMix {
    pub fn single(kind: PickerKind) -> PickerMix {
        PickerMix {
            rotation: vec![kind],
        }
    }

    /// Which picker the `n`th guess of the search gets.
    pub fn for_guess(&self, n: usize) -> PickerKind {
        self.rotation[n % self.rotation.len()]
    }
}

impl std::str::FromStr for PickerMix {
    type Err = String;

    /// `disagreement:3,random:1`, or a bare `random`, or anything in between: a comma-separated
    /// list of picker names, each with an optional `:count` that defaults to 1.
    fn from_str(spec: &str) -> Result<PickerMix, String> {
        use clap::ValueEnum;

        let mut rotation = vec![];
        for term in spec.split(',') {
            let term = term.trim();
            let (name, count) = match term.split_once(':') {
                Some((name, count)) => {
                    let count: usize = count
                        .trim()
                        .parse()
                        .map_err(|_| format!("{count:?} isn't a count, in {term:?}"))?;
                    if count == 0 {
                        return Err(format!("a count of 0 leaves nothing to pick, in {term:?}"));
                    }
                    (name.trim(), count)
                }
                None => (term, 1),
            };

            let kind = PickerKind::from_str(name, /*ignore_case=*/ true)
                .map_err(|_| format!("no such picker: {name:?}"))?;

            if rotation.len() + count > MAX_ROTATION {
                return Err(format!("more than {MAX_ROTATION} pickers in one rotation"));
            }
            rotation.extend(std::iter::repeat_n(kind, count));
        }

        if rotation.is_empty() {
            return Err("no pickers at all".to_string());
        }

        Ok(PickerMix { rotation })
    }
}

impl std::fmt::Display for PickerMix {
    /// The spec `from_str` would parse back into this rotation, runs collapsed into counts.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut first = true;
        for (kind, run) in self
            .rotation
            .chunk_by(|a, b| a == b)
            .map(|r| (r[0], r.len()))
        {
            if !first {
                write!(f, ",")?;
            }
            first = false;
            write!(f, "{}:{}", kind.name(), run)?;
        }
        Ok(())
    }
}

/// Build the picker `kind` names and ask it for a guess. Each picker gets built fresh for the
/// state it's about to look at; see `GuessPicker`.
pub(super) fn pick_guess<C: Clue, K: GridKind>(
    kind: PickerKind,
    state: &SolveState<'_, C>,
    linear_ctx: &SolveContext<'_, '_, C, K>,
) -> Option<(usize, Color)> {
    match kind {
        PickerKind::First => First::new(state, linear_ctx).pick(state, linear_ctx),
        PickerKind::Edge => Edge::new(state, linear_ctx).pick(state, linear_ctx),
        PickerKind::Middle => Middle::new(state, linear_ctx).pick(state, linear_ctx),
        PickerKind::Random => Random::new(state, linear_ctx).pick(state, linear_ctx),
        PickerKind::Disagreement => Disagreement::new(state, linear_ctx).pick(state, linear_ctx),
        PickerKind::Neighbors => Neighbors::new(state, linear_ctx).pick(state, linear_ctx),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::import::bw_palette;
    use crate::puzzle::{Nono, Puzzle};
    use crate::solve::grid_solve::SolveOptions;

    /// What `Neighbors` counts: the cells adjacent along a lane, and the lane ends where the
    /// puzzle runs out instead.
    #[test]
    fn a_neighborhood_is_four_lane_neighbors_and_the_edges_among_them() {
        // A 3x3 grid; the clues don't matter, only the shape does.
        let clue = vec![Nono {
            color: Color(1),
            count: 1,
        }];
        let puzzle = Puzzle::square(bw_palette(), vec![clue.clone(); 3], vec![clue; 3]);
        let mut line_cache = None;
        let options = SolveOptions::default();
        let ctx = SolveContext::new(&puzzle, &mut line_cache, &options);

        // Two lanes cross every cell, so every cell has four sides, edges included.
        let sides = neighborhood(&ctx, /*edge=*/ 1.0, |_| true);
        assert_eq!(sides, vec![4.0; 9]);

        // Corners are against two of them, the middles of the sides one, and the center none.
        let edges = neighborhood(&ctx, /*edge=*/ 1.0, |_| false);
        #[rustfmt::skip]
        let want = vec![
            2.0, 1.0, 2.0,
            1.0, 0.0, 1.0,
            2.0, 1.0, 2.0,
        ];
        assert_eq!(edges, want);
    }

    /// Every spelling `--picker` accepts, and what it means. The `:count`s expand into a
    /// rotation, and `Display` puts them back together.
    #[test]
    fn picker_mixes_parse_and_print() {
        let mix: PickerMix = "disagreement:3,random:1".parse().unwrap();
        assert_eq!(
            mix.rotation,
            vec![
                PickerKind::Disagreement,
                PickerKind::Disagreement,
                PickerKind::Disagreement,
                PickerKind::Random
            ]
        );
        // Four guesses in, the rotation comes back around.
        let picked: Vec<PickerKind> = (0..5).map(|n| mix.for_guess(n)).collect();
        assert_eq!(
            picked,
            [mix.rotation.clone(), vec![PickerKind::Disagreement]].concat()
        );

        // A count of 1 is what a bare name means, and `Display` writes the counts out.
        assert_eq!(
            "random".parse::<PickerMix>().unwrap(),
            PickerMix::single(PickerKind::Random)
        );
        assert_eq!(mix.to_string(), "disagreement:3,random:1");
        assert_eq!(
            " first , edge:2 ".parse::<PickerMix>().unwrap().to_string(),
            "first:1,edge:2"
        );

        // A rotation can name the same picker twice; `Display` only collapses adjacent runs.
        let alternating: PickerMix = "first,random,first".parse().unwrap();
        assert_eq!(alternating.to_string(), "first:1,random:1,first:1");

        for bad in [
            "",
            "nonesuch",
            "random:0",
            "random:x",
            "first,",
            "random:9999",
        ] {
            assert!(
                bad.parse::<PickerMix>().is_err(),
                "{bad:?} should not parse"
            );
        }
    }

    /// `PickerKind::name` is written out by hand; this is what keeps it honest.
    #[test]
    fn picker_names_round_trip() {
        for kind in PickerKind::ALL {
            let mix = PickerMix::single(kind);
            assert_eq!(mix.to_string().parse::<PickerMix>().unwrap(), mix);
        }
    }
}
