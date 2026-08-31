//! Deciding *which node* the search should work on next.
//!
//! `bt_solve`'s queue is a best-first one over hypotheses, so the scoring function here is what
//! gives the search its shape — dive, probe, or something in between. It's the counterpart of
//! `bt_picking.rs` (which decides where to guess *within* a node), and it's split out for the same
//! reason: it's a knob worth benchmarking. See `bench-pbnsolve --mode backtrack --scorer`.

use crate::puzzle::PartialSolution;

/// Everything a scorer knows besides the node itself.
pub(super) struct ScoreCtx<'a> {
    pub kind: ScoreKind,
    /// The size of the whole grid, for scorers that measure progress as a fraction.
    pub total_cells: usize,
    /// The solution the search has already found, if any (see `Terms::rediscovery`).
    pub solution_found: Option<&'a PartialSolution>,
}

/// The raw measurements a node offers, gathered once so the formulas below can read like
/// formulas. Every field is "bigger is worse" except where noted.
pub(super) struct Terms {
    /// Unknown cells left in this node's grid.
    pub cells_left: f32,
    /// Unknown cells the node's *parent* had, so `parent_cells_left - cells_left` is what this
    /// node's guess bought.
    pub parent_cells_left: f32,
    /// Summed candidate colors over the unknown cells: a finer `cells_left` that notices a
    /// multicolor cell narrowing from four candidates to two.
    pub candidates: f32,
    /// How deep the hypothesis stack is.
    pub levels: f32,
    /// Guesses already made *at* this node.
    pub explored: f32,
    /// Whether the node's first guess agrees with the solution already found — such a node is on
    /// its way to rediscovering it, which settles nothing.
    pub rediscovering: bool,
    /// Cells in the whole grid.
    pub total_cells: f32,
    /// Whether the search has a solution in hand already, and so has moved on from finding one
    /// to proving there isn't a second.
    pub solution_known: bool,
}

impl Terms {
    /// The baseline's depth penalty: doubling per level, but only down to depth 5 — past there
    /// every node is buried anyway, and what's left to do is order them, which a gentle slope
    /// does as well as a cliff.
    fn stepped_depth(&self) -> f32 {
        let levels = self.levels;
        5.0 * 2.0_f32.powi(levels.min(5.0) as i32) + if levels > 5.0 { levels * 5.0 } else { 0.0 }
    }

    /// The fraction of the grid still unknown, in `0.0..=1.0`.
    fn unknown_frac(&self) -> f32 {
        self.cells_left / self.total_cells.max(1.0)
    }

    /// The fraction of its parent's unknown cells this node's guess resolved, in `0.0..=1.0`.
    fn progress_frac(&self) -> f32 {
        if self.parent_cells_left <= 0.0 {
            return 0.0;
        }
        (self.parent_cells_left - self.cells_left) / self.parent_cells_left
    }
}

/// How `bt_solve` orders its queue of hypotheses. Lower scores are searched first.
#[derive(clap::ValueEnum, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ScoreKind {
    /// Mostly "fewest unknown cells wins", with a doubling penalty per hypothetical level.
    #[default]
    Baseline,
    /// `Baseline`, but with the distance measured as a fraction of the grid, so the depth penalty
    /// means the same thing on a 50x50 as it does on a 5x5.
    Normalized,
    /// Deepest node first: a plain depth-first dive, backing out only on a contradiction.
    Dfs,
    /// Shallowest node first: probe every level-1 guess before opening a level-2 one.
    Bfs,
    /// Prefer the node whose guess taught the most, as a fraction of what its parent didn't know.
    Progress,
    /// Fewest unknown cells wins, full stop — no depth penalty at all.
    Flat,
    /// `Normalized`, but counting candidate colors rather than unknown cells.
    Candidates,
    /// `Dfs`, but a node that has already been guessed at several times loses its place.
    DfsRestless,
    /// `Normalized` with a much heavier depth penalty: dig only when the shallow options are
    /// genuinely worse.
    Shallow,
    /// `Bfs`, but a node yields its turn once it has made about ten guesses, so probing a wide
    /// grid doesn't fork a child per cell before anything else gets a look in.
    Probe10,
    /// `Probe10` with a budget of about thirty guesses per node.
    Probe30,
    /// `Probe10` with a budget of about a hundred guesses per node.
    Probe100,
    /// Shallow-first, but a node that has learned a great deal can still jump the queue: the
    /// depth and distance terms are on the same scale rather than one dominating.
    Blend,
    /// Two phases, because the search has two jobs: dive (`Progress`) until it holds a solution,
    /// then BFS (`Bfs`) to prove there isn't a second one.
    Hunt,
    /// `Hunt` with the phases the other way round, as a control.
    Refute,
}

impl ScoreKind {
    pub const ALL: [ScoreKind; 15] = [
        ScoreKind::Baseline,
        ScoreKind::Normalized,
        ScoreKind::Dfs,
        ScoreKind::Bfs,
        ScoreKind::Progress,
        ScoreKind::Flat,
        ScoreKind::Candidates,
        ScoreKind::DfsRestless,
        ScoreKind::Shallow,
        ScoreKind::Probe10,
        ScoreKind::Probe30,
        ScoreKind::Probe100,
        ScoreKind::Blend,
        ScoreKind::Hunt,
        ScoreKind::Refute,
    ];
}

impl ScoreKind {
    /// The name `--scorer` spells this kind with, straight from the `ValueEnum` derive, so the
    /// benchmark can pass its own choice down to the child process it runs the search in.
    pub fn flag_name(self) -> String {
        use clap::ValueEnum;
        self.to_possible_value()
            .expect("every ScoreKind is a possible value")
            .get_name()
            .to_string()
    }
}

/// The score itself: lower is searched sooner.
pub(super) fn score(kind: ScoreKind, t: &Terms) -> f32 {
    // Every formula wants to push a node that's merely rediscovering the known solution to the
    // back; how far back depends on the scale the rest of the formula works in.
    let rediscovery = |scale: f32| if t.rediscovering { scale } else { 0.0 };

    match kind {
        ScoreKind::Baseline => {
            t.cells_left + t.stepped_depth() + rediscovery(500.0) + t.explored * 3.0
        }
        ScoreKind::Normalized => {
            1000.0 * t.unknown_frac() + t.stepped_depth() + rediscovery(500.0) + t.explored * 3.0
        }
        ScoreKind::Dfs => -1000.0 * t.levels + t.unknown_frac() + rediscovery(1e6),
        ScoreKind::Bfs => 1000.0 * t.levels + t.unknown_frac() + rediscovery(1e6),
        ScoreKind::Progress => {
            1000.0 * (1.0 - t.progress_frac())
                + t.stepped_depth()
                + rediscovery(500.0)
                + t.explored * 3.0
        }
        ScoreKind::Flat => t.cells_left + rediscovery(500.0) + t.explored * 3.0,
        ScoreKind::Candidates => {
            1000.0 * (t.candidates / t.total_cells.max(1.0))
                + t.stepped_depth()
                + rediscovery(500.0)
                + t.explored * 3.0
        }
        ScoreKind::DfsRestless => {
            -1000.0 * t.levels + t.explored * 300.0 + t.unknown_frac() + rediscovery(1e6)
        }
        ScoreKind::Shallow => {
            100.0 * t.unknown_frac() + t.stepped_depth() + rediscovery(500.0) + t.explored * 3.0
        }
        // A breadth budget: `explored * (1000 / budget)` costs a node its whole level's worth of
        // advantage once it has spent the budget, so it steps aside and lets the next one probe.
        ScoreKind::Probe10 => probe(t, 10.0),
        ScoreKind::Probe30 => probe(t, 30.0),
        ScoreKind::Probe100 => probe(t, 100.0),
        ScoreKind::Blend => {
            200.0 * t.levels + 1000.0 * t.unknown_frac() + rediscovery(1e4) + t.explored * 3.0
        }
        ScoreKind::Hunt if !t.solution_known => score(ScoreKind::Progress, t),
        ScoreKind::Hunt => 1000.0 * t.levels + t.unknown_frac() + rediscovery(1e6), // = BFS
        ScoreKind::Refute if t.solution_known => score(ScoreKind::Progress, t),
        ScoreKind::Refute => probe(t, 100.0),
    }
}

/// Shallowest-first, with each node allowed about `budget` guesses before it drops behind the
/// nodes a level below it.
fn probe(t: &Terms, budget: f32) -> f32 {
    1000.0 * t.levels
        + (1000.0 / budget) * t.explored
        + t.unknown_frac()
        + if t.rediscovering { 1e6 } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `flag_name` is what the benchmark hands its child process; a name that doesn't parse back
    /// would silently run the child with the default scorer instead.
    #[test]
    fn scorer_names_round_trip() {
        use clap::ValueEnum;
        for kind in ScoreKind::ALL {
            let name = kind.flag_name();
            assert_eq!(
                ScoreKind::from_str(&name, /*ignore_case=*/ true),
                Ok(kind),
                "{name:?} didn't parse back"
            );
        }
    }

    /// Nothing here may hand the queue a `NaN`: `Score`'s `Ord` unwraps a `partial_cmp`, so one
    /// would take the whole search down. An empty grid is the shape most likely to produce one.
    #[test]
    fn no_scorer_produces_a_nan() {
        let empty = Terms {
            cells_left: 0.0,
            parent_cells_left: 0.0,
            candidates: 0.0,
            levels: 0.0,
            explored: 0.0,
            rediscovering: false,
            total_cells: 0.0,
            solution_known: false,
        };
        for kind in ScoreKind::ALL {
            assert!(!score(kind, &empty).is_nan(), "{kind:?} scored NaN");
        }
    }
}
