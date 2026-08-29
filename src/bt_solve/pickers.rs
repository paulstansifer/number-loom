//! Deciding *where* to guess, once line logic has stalled and `bt_solve` has to branch.
//!
//! A `GuessPicker` rates the guesses available in one node and names the best one; `PickerMix`
//! says which picker each guess gets, so a search can rotate through several. This is the part
//! of the search worth experimenting with — see `bench-pbnsolve --mode backtrack --picker` — so
//! it lives apart from the search itself, which doesn't care how the guesses get chosen.

use crate::{
    bt_solve::{BtSolveState, Score},
    geometry::GridKind,
    grid_solve::SolveContext,
    puzzle::{BACKGROUND, Clue, Color},
};

/// Decides which cell to guess at, and what color to guess.
///
/// `rate` runs once per candidate guess — that's every unknown cell times every color it could
/// be — so anything that's the same for all of a node's guesses belongs in `new`, which runs
/// once per node, just before the guesses it's about to rate.
trait GuessPicker: Sized {
    /// Precompute whatever `rate` shouldn't be doing over and over. `state` is the node whose
    /// guesses this picker will rate; a picker that only cares about the puzzle's shape can
    /// ignore it.
    fn new<C: Clue, K: GridKind>(
        state: &BtSolveState<'_, C>,
        linear_ctx: &SolveContext<'_, '_, C, K>,
    ) -> Self;

    /// Score guesses against each other. Note that this is totally different than *node* scores!
    fn rate<C: Clue, K: GridKind>(
        &self,
        state: &BtSolveState<'_, C>,
        linear_ctx: &SolveContext<'_, '_, C, K>,
        guess: (usize, Color),
    ) -> Score;

    /// Pick the lowest-scoring choice that's a valid guess
    fn pick<C: Clue, K: GridKind>(
        &self,
        state: &BtSolveState<'_, C>,
        linear_ctx: &SolveContext<'_, '_, C, K>,
    ) -> Option<(usize, Color)> {
        let idxed_cells = state.knowledge.grid.iter().enumerate();
        let uncertain_cells = idxed_cells.filter(|(_, cell)| !cell.is_known());
        let options = uncertain_cells
            .flat_map(|(idx, cell)| cell.can_be_iter().map(move |color| (idx, color)));
        let unused_options = options.filter(|guess| !state.guesses_explored.contains(guess));
        // `min_by_key` keeps the first of a tie, like the stable sort this used to do.
        unused_options.min_by_key(|guess| self.rate(state, linear_ctx, *guess))
    }
}

struct First;

impl GuessPicker for First {
    fn new<C: Clue, K: GridKind>(_: &BtSolveState<'_, C>, _: &SolveContext<'_, '_, C, K>) -> First {
        First
    }

    fn rate<C: Clue, K: GridKind>(
        &self,
        _: &BtSolveState<'_, C>,
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
        _: &BtSolveState<'_, C>,
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
        _: &BtSolveState<'_, C>,
        _: &SolveContext<'_, '_, C, K>,
        (idx, _): (usize, Color),
    ) -> Score {
        Score(self.edginess[idx])
    }
}

/// `Edge`, backwards: guess in the middle of the lanes, where the clues have the least to say.
/// Here to find out whether `Edge` is a bad heuristic or a good one pointed the wrong way.
struct Middle(Edge);

impl GuessPicker for Middle {
    fn new<C: Clue, K: GridKind>(
        state: &BtSolveState<'_, C>,
        linear_ctx: &SolveContext<'_, '_, C, K>,
    ) -> Middle {
        Middle(Edge::new(state, linear_ctx))
    }

    fn rate<C: Clue, K: GridKind>(
        &self,
        state: &BtSolveState<'_, C>,
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
        state: &BtSolveState<'_, C>,
        _: &SolveContext<'_, '_, C, K>,
    ) -> Random {
        use std::hash::BuildHasher;

        // `RandomState`'s keys differ from one instance to the next, so this is a fresh stream
        // per node, and a different search every run.
        let seed =
            std::collections::hash_map::RandomState::new().hash_one(state.knowledge.cells_left);

        Random {
            rng: std::cell::Cell::new(seed | 1), // xorshift never leaves zero
        }
    }

    fn rate<C: Clue, K: GridKind>(
        &self,
        _: &BtSolveState<'_, C>,
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
        state: &BtSolveState<'_, C>,
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
                match state.knowledge.grid[*cell_idx as usize].known_or() {
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
        _: &BtSolveState<'_, C>,
        _: &SolveContext<'_, '_, C, K>,
        (idx, color): (usize, Color),
    ) -> Score {
        Score(self.ratings[idx * self.stride + color.0 as usize])
    }
}

/// Which `GuessPicker` `backtrack_solve` uses. The search's shape depends entirely on where it
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
}

impl PickerKind {
    pub const ALL: [PickerKind; 5] = [
        PickerKind::First,
        PickerKind::Edge,
        PickerKind::Middle,
        PickerKind::Random,
        PickerKind::Disagreement,
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickerMix {
    /// The rotation, with the weights already spelled out: `disagreement:3,random:1` is stored
    /// as three `Disagreement`s and a `Random`, and `for_guess` just indexes it.
    rotation: Vec<PickerKind>,
}

impl Default for PickerMix {
    fn default() -> PickerMix {
        use crate::bt_solve::pickers::PickerKind::{Disagreement, Random};
        PickerMix {
            rotation: vec![Disagreement, Disagreement, Disagreement, Random],
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
    pub(super) fn for_guess(&self, n: usize) -> PickerKind {
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
/// node it's about to look at; see `GuessPicker`.
pub(super) fn pick_guess<C: Clue, K: GridKind>(
    kind: PickerKind,
    state: &BtSolveState<'_, C>,
    linear_ctx: &SolveContext<'_, '_, C, K>,
) -> Option<(usize, Color)> {
    match kind {
        PickerKind::First => First::new(state, linear_ctx).pick(state, linear_ctx),
        PickerKind::Edge => Edge::new(state, linear_ctx).pick(state, linear_ctx),
        PickerKind::Middle => Middle::new(state, linear_ctx).pick(state, linear_ctx),
        PickerKind::Random => Random::new(state, linear_ctx).pick(state, linear_ctx),
        PickerKind::Disagreement => Disagreement::new(state, linear_ctx).pick(state, linear_ctx),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
