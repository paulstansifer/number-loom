//! Deciding *which node* the search should work on next.
//!
//! `bt_solve`'s queue is a best-first one over hypotheses, so the scoring function here is what
//! gives the search its shape — dive, probe, or something in between. It's the counterpart of
//! `bt_picking.rs` (which decides where to guess *within* a node), and it's split out for the same
//! reason: it's a knob worth benchmarking. See `bench-pbnsolve --mode backtrack --scorer`.

use crate::puzzle::PartialSolution;

/// Everything a scorer knows besides the node itself.
pub(super) struct ScoreCtx<'a> {
    pub kind: ScorerPair,
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
/// One node-ordering formula. What the solver actually runs is a `ScorerPair` of these — there
/// is deliberately no `Default` here, because a default on a single kind reads as "this is what
/// ships", which is `ScorerPair::default()`'s job to say.
#[derive(clap::ValueEnum, Clone, Copy, PartialEq, Eq, Debug)]
pub enum ScoreKind {
    /// Mostly "fewest unknown cells wins", with a doubling penalty per hypothetical level.
    Baseline,
    /// `Baseline`, but with the distance measured as a fraction of the grid, so the depth penalty
    /// means the same thing on a 50x50 as it does on a 5x5.
    Normalized,
    /// Deepest node first: a plain depth-first dive, backing out only on a contradiction.
    Dfs,
    /// Shallowest node first: probe every level-1 guess before opening a level-2 one.
    Bfs,
    /// Puts strong value on proportion of the grid discovered
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
}

impl ScoreKind {
    pub const ALL: [ScoreKind; 13] = [
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

/// Which scorer the search uses, in each of its two phases.
///
/// The search has two jobs, and they don't want the same ordering. Until it holds a solution it
/// is *hunting* for one, and wants whatever finds one soonest; once it has one, the job changes
/// to proving no second solution exists, which is a different shape of search. Naming the two
/// separately means any pairing can be tried, rather than only the handful that used to be
/// spelled out as their own `ScoreKind`s: the old `Hunt` is `progress/bfs`, `Refute` is
/// `probe100/progress`, `Dive` is `dfs/probe30`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ScorerPair {
    /// Used while the search is still looking for a solution.
    pub initial: ScoreKind,
    /// Used once a solution is in hand and the job is proving it unique.
    pub confirming: ScoreKind,
}

impl Default for ScorerPair {
    /// `Progress` to hunt, `Bfs` to confirm: 21 of the 31 puzzles `bench-pbnsolve --mode
    /// backtrack` runs decide within ten seconds, against 13 for the `Baseline` this replaced.
    /// It wins nine of them outright and loses only `webpbn-10810`, which `Baseline` gets in
    /// 0.60s. The split is the point — hunting for a solution and proving no second one exists
    /// want different orderings, and no single scorer was good at both.
    fn default() -> ScorerPair {
        ScorerPair {
            initial: ScoreKind::Progress,
            confirming: ScoreKind::Bfs,
        }
    }
}

impl ScorerPair {
    /// The same scorer for both phases, which is what a bare `--scorer baseline` means.
    pub fn single(kind: ScoreKind) -> ScorerPair {
        ScorerPair {
            initial: kind,
            confirming: kind,
        }
    }

    /// Which of the two applies; `solution_known` is the phase.
    fn for_phase(&self, solution_known: bool) -> ScoreKind {
        if solution_known {
            self.confirming
        } else {
            self.initial
        }
    }
}

impl std::str::FromStr for ScorerPair {
    type Err = String;

    /// `progress/bfs` — the scorer to hunt with, then the one to confirm with. A bare name, like
    /// `baseline`, uses that scorer for both phases.
    fn from_str(spec: &str) -> Result<ScorerPair, String> {
        use clap::ValueEnum;

        let one = |name: &str| {
            let name = name.trim();
            ScoreKind::from_str(name, /*ignore_case=*/ true)
                .map_err(|_| format!("no such scorer: {name:?}"))
        };

        match spec.split_once('/') {
            Some((initial, confirming)) => Ok(ScorerPair {
                initial: one(initial)?,
                confirming: one(confirming)?,
            }),
            None => Ok(ScorerPair::single(one(spec)?)),
        }
    }
}

impl std::fmt::Display for ScorerPair {
    /// The spec `from_str` would parse back into this pair; a pair that uses one scorer for both
    /// phases prints as the bare name.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.initial == self.confirming {
            write!(f, "{}", self.initial.flag_name())
        } else {
            write!(
                f,
                "{}/{}",
                self.initial.flag_name(),
                self.confirming.flag_name()
            )
        }
    }
}

/// The score itself: lower is searched sooner. Picks the phase's scorer out of `pair` — see
/// `ScorerPair` for why there are two.
pub(super) fn score(pair: ScorerPair, t: &Terms) -> f32 {
    score_one(pair.for_phase(t.solution_known), t)
}

/// One scorer's formula, with the phase already decided.
fn score_one(kind: ScoreKind, t: &Terms) -> f32 {
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
                + t.explored * 0.5
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
            assert!(!score_one(kind, &empty).is_nan(), "{kind:?} scored NaN");
        }
    }

    /// The spec `--scorer` takes has to survive the round trip the benchmark puts it through
    /// (parse, hand to a child process as a string, parse again), for both shapes it can take.
    #[test]
    fn scorer_pairs_round_trip() {
        use std::str::FromStr;

        for initial in ScoreKind::ALL {
            for confirming in ScoreKind::ALL {
                let pair = ScorerPair {
                    initial,
                    confirming,
                };
                let spelled = pair.to_string();
                assert_eq!(
                    ScorerPair::from_str(&spelled),
                    Ok(pair),
                    "{spelled:?} didn't parse back"
                );
            }
        }
    }

    /// A bare name means both phases, and prints back as the bare name rather than `x/x`.
    #[test]
    fn a_bare_scorer_name_means_both_phases() {
        use std::str::FromStr;

        let both = ScorerPair::from_str("progress").unwrap();
        assert_eq!(both, ScorerPair::single(ScoreKind::Progress));
        assert_eq!(both.to_string(), "progress");

        let split = ScorerPair::from_str("progress/bfs").unwrap();
        assert_eq!(split.initial, ScoreKind::Progress);
        assert_eq!(split.confirming, ScoreKind::Bfs);
        assert_eq!(split.to_string(), "progress/bfs");

        assert!(ScorerPair::from_str("progress/nope").is_err());
        assert!(ScorerPair::from_str("nope").is_err());
    }

    /// The pair is what makes the phases separable: the same node scores differently depending on
    /// whether a solution has been found yet. This is the old `Hunt`, spelled as a pair.
    #[test]
    fn a_pair_switches_scorer_when_a_solution_is_found() {
        let hunt = ScorerPair {
            initial: ScoreKind::Progress,
            confirming: ScoreKind::Bfs,
        };
        let terms = |solution_known| Terms {
            cells_left: 20.0,
            parent_cells_left: 40.0,
            candidates: 40.0,
            levels: 3.0,
            explored: 1.0,
            rediscovering: false,
            total_cells: 100.0,
            solution_known,
        };

        assert_eq!(
            score(hunt, &terms(false)),
            score_one(ScoreKind::Progress, &terms(false))
        );
        assert_eq!(
            score(hunt, &terms(true)),
            score_one(ScoreKind::Bfs, &terms(true))
        );
    }
}
