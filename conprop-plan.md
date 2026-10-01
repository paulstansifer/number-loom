# Rebuilding the conprop work, step by step

A checklist for reimplementing the changes in `solve/conprop.rs` and its neighbours from scratch.
Every step here exists in the current tree, so you can diff against it when you want to — but the
order is chosen so that each step is *verifiable on its own* before the next one depends on it, which
is not the order I originally did them in.

Two things to internalize before starting, because both cost me real time:

- **Measure before changing anything.** Nearly every conclusion below contradicts a plausible guess.
  The picker turned out to cost more than the search; restarts turned out to do literally nothing;
  the "obvious" cheap explanation turned out to be wider than what it replaced.
- **The only real correctness check these paths have is `--mode conprop` agreeing with pbnsolve about
  `unique` vs `multiple`.** `solver_fuzzer` exercises line logic only. It caught a soundness bug of
  mine that nothing else would have.

---

## Phase 0 — make measurement possible

### [x] 0.1 Seed the picker's RNG

`Picker::from_situation` shuffled with a fresh `rand::thread_rng()` per call. Thread it through from
a single `StdRng` on the search state, seeded from a new `SolveOptions::rng_seed`, and add `--seed`.

**Why first:** without it nothing downstream is measurable. Same binary, same puzzle, nine runs:
`webpbn-04645` ranged 0.102–0.270s and `webpbn-00803` 0.029–0.208s. You cannot tell a 1.3x win from
noise in that.

**Verify:** three runs of one puzzle give times within a percent of each other.

**Then learn the other half of the lesson:** determinism does not remove the variance, it just makes
it reproducible. On `webpbn-color-02814`, seeds 1–14 at a 20s budget: seed 11 finishes in **2.24s**,
seeds 3 and 4 in ~18s, and the other eleven not at all. The runtime distribution is heavy-tailed. Any
single row of the benchmark is one sample. **Sweep several seeds before believing a change helped.**

### [ ] 0.2 A stats struct on the search

Counters for: guesses, conflicts (and whether line logic or a nogood found each), nogoods learned
with their total literal count and the trail depth at each conflict, levels unwound, unit nogoods,
deductions made with no guess outstanding, max depth, propagate rounds, nogood visits, pickers built,
line-cache hits/misses, and a wall-clock split (picking / deriving / total).

Add a `--solve-stats` CLI flag that prints them, and columns plus a `--verbose` per-puzzle dump in
`--mode conprop`. Aim for something you can read next to `pbnsolve -t`'s block.

**Two counters matter more than the rest, because the existing `Report` hides the work:**

- **Replayed line solves.** The conflict explanation clones the state and re-solves lanes; none of
  that reaches `Report::solve_counts`. It is 50–96% of all line logic.
- **Picker lane skims.** `Picker::rescore` calls `grid_solve::fixed_clues`, which skims *every* lane,
  uncached and uncounted. On `webpbn-04645` that was 7,920 lane skims against the search's own 7,535.

To count the replay, reset the clone's stats after cloning and fold the deltas back into named
`replay_*` fields. To count the picker, put a `scoring_passes` counter on `Picker` and multiply the
delta by the lane count.

**Verify:** `Cache Hits` denominator should exactly equal your scrub count (see 2.1 for why).

### [ ] 0.3 A guess budget

`SolveOptions::max_guesses` plus `--max-guesses N`, honoured in the search loop: stop and report what
is known. Only conprop needs to honour it.

**Why:** the interesting puzzles are the ones that never finish, and killing the process throws the
counters away with it. Every diagnosis in Phase 1 came from `--max-guesses 3000` on a puzzle that
otherwise runs forever.

### [ ] 0.4 Fix the pbnsolve parser

`Backtracking: 0 guesses, 0 backtracks` becomes `Backtracking: 762 probes, 146 guesses, 146
backtracks` once probing is on — which is pbnsolve's default. Reading that line positionally puts
probes in `guesses` and guesses in `backtracks`. Parse by label. Also worth grabbing its
`Cache Hits:` line, and keeping its whole `-t` block so `--verbose` can print it verbatim.

**Verify:** a test with both forms of the line. Real captured output beats invented output.

---

## Phase 1 — read the numbers before writing code

Do this as a reading exercise, not a coding one. Run `--mode conprop --verbose` and
`--solve-stats --max-guesses 3000` on a handful of puzzles and find these five things yourself.

### [ ] 1.1 Our search is much smaller than pbnsolve's; our per-operation cost is much higher

`webpbn-04645`: we do essentially the same total lane work as pbnsolve (≈53,300 operations vs 53,551)
and take **14x** as long. `webpbn-00803`: we do 22x *less* work and win only 1.8x. `webpbn-10810`: 264x
less work, 30x faster. Consistent story — our search is better, our constant factor is 7–14x worse.

### [ ] 1.2 The picker, not the search, is the top cost

30–86% of the clock on nearly every puzzle. And each picker serves about **1.7 picks** (54 picks / 32
pickers on `webpbn-04645`) while sorting *every* `(cell, colour)` candidate twice.

### [ ] 1.3 Nogood width tracks trail depth

`17.6` literals against a trail `18.9` deep; `23.7` against `24.6`; `3.8` against `5.0`. The clause
names nearly every guess on the trail. So it backjumps ~2 levels and, needing 17 of 18 specific cells
to hold at once, never fires again. The database is write-only.

Two easy wrong conclusions to avoid here:

- *"The minimizer isn't working."* It is, nearly optimally — see 1.5.
- *"The clause isn't asserting."* It is. Count it: essentially 100% of them assert. The rotation that
  puts the last guess first is what makes that true.

### [ ] 1.4 The timeouts differ by *rate of permanent progress*

A one-literal nogood is the only clause that forces something at the root and so permanently shrinks
the puzzle. Count them as a share of conflicts:

|                          | conflicts | unit nogoods | rate      |
| ------------------------ | --------- | ------------ | --------- |
| webpbn-04645 (solves)    | 29        | 8            | **28%**   |
| color-02073 (solves)     | 76        | 12           | **16%**   |
| webpbn-03541 (times out) | 1136      | 2            | **0.18%** |
| color-02814 (times out)  | 1485      | 2            | **0.13%** |

A 40–200x difference. Combined with 1.3 this says the top ~18 guesses are frozen for the whole run:
the only thing that unwinds a guess is a conflict whose clause doesn't name it.

**Do not conclude "the picker is catastrophically worse".** On every puzzle we finish we use *fewer*
guesses than pbnsolve (71 vs 180; 143 vs 285; 42 vs 996).

### [ ] 1.5 Reordering the replay does not help

Worth doing as an experiment so you believe it. Replay newest-first instead of last-then-oldest, with
the backjump level read off the clause rather than off the replay position. Width barely moves
(17.6→15.8 on one puzzle, 12.2→18.5 on another) and backjumps collapse to exactly one level. The
contradiction genuinely needs nearly every decision. **Minimizing over decisions is at its floor;
the way out is to stop restricting clauses to decisions.**

---

## Phase 2 — the cheap structural wins

Independent of all the clause-learning work below. Do them first; they are small.

### [x] 2.1 Make skims use the line cache

`op_or_cache` wraps only `SolveMode::Scrub`. `skim_line` goes straight through, and `fixed_clues`
calls `skim_to_find_fixed_clues` uncached. So on `webpbn-04645`: 22,884 line operations, of which
3,798 (17%) are even *eligible* for the cache and 2,183 (9.5%) hit. pbnsolve caches essentially
everything and hits 90%.

Given how much of our line work is repeated — the replay walking the same trail, the picker sweeping
unchanged lanes — this should hit hard. There is already a TODO for it at the top of `conprop.rs`.

**Watch out:** the cache key is a freshly allocated `(Vec<C>, Vec<u32>)` per lookup. At these lookup
rates that allocation is plausibly on the hot path. I never profiled it (`perf` was blocked by
`perf_event_paranoid` here), so treat it as a hypothesis, not a finding.

(Tried; seems to barely affect performance. Perhaps lookup time is similar to actually performing a skim?)

### [x] 2.2 Stop the picker scoring every lane twice per picker

`from_situation` calls `rescore`, then the first `pick` calls it again because `picks_made % 5 == 0`
is true at zero. The counter confirms it: ~2.05 scoring passes per picker.

**Measured** (median of 9 runs): `webpbn-04645` 0.130→0.077s, `color-02817` 0.493→0.332s,
`webpbn-00803` 0.035→0.027s, `webpbn-01694` 0.299→0.259s. Worth 1.15–1.7x for a one-line change.

### [/] 2.3 The rest of the picker

In rough order of value over effort:

- **Bounded selection** instead of two full sorts. You need the top one or two candidates, not a
  total order over thousands. `select_nth_unstable` or a small heap. There is a TODO for this.
  **Done** (two `BinaryHeap`s, popped lazily). On seed/puzzle pairs where the search came out
  identical (same skim counts), 1.2–1.5x: `color-02817` 0.828→0.649s, `webpbn-01694` 0.417→0.338s,
  `webpbn-04645` 0.085→0.061s, `webpbn-00803` 0.013→0.008s. Ties at the 5th-pick rescore now break by
  the original shuffle rather than by the previous order, so some searches differ.
- **Rescore only at shallow levels.** Deep levels get backjumped away in ~2 levels, so their scoring
  is thrown out. There is a TODO speculating exactly this, and the depth numbers support it.
- **Incremental scoring.** Consecutive pickers see a grid differing by one guess plus propagation, so
  only touched lanes need rework. Most work, highest ceiling.
- Also: `from_situation` builds a fresh `rand::thread_rng()` per call (removed by 0.1, but note it).

---

## Phase 3 — reasons over cells instead of over guesses

This is the core idea. The existing clause is a disjunction over *decisions*, which is why it is as
wide as the trail. A clause over *derived cell values* can be narrow, because it describes the local
cause rather than the route taken.

### [x] 3.1 Record which lane a contradiction came from

Add `SolveState::last_lane: Option<LaneIdx>`, set just before each line solve. A contradiction
arrives as an `Err` from deep inside the solve with no room to say where it came from.

**The soundness trap, and it bit me:** `last_lane` is where the solver *was*, not necessarily where
the contradiction is — the error can come from `learn`, or from a nogood. **Always re-check that the
lane really is unsatisfiable before explaining it.** Minimizing against a satisfiable lane keeps every
candidate and yields a clause asserting a contradiction that does not exist. Mine made
`webpbn-01694` and `color-01503` report `multiple` for uniquely-solvable puzzles. It fires on about 3
of 211 and 7 of 54 conflicts — rare enough to miss, frequent enough to break things.

### [x] 3.2 Minimize a lane reason by deletion

To explain a contradiction in lane L: take the cells of L that the trail narrowed since the root, and
drop each in turn — restoring it to **its root state**, not to blank — re-running `exhaust_line` each
time. Keep a cell only if dropping it makes the lane satisfiable again. Cells absent from the trail
were settled at the root and hold in every branch, so they belong in no clause.

**The other soundness trap, also mine:** my first version treated only *fully known* cells as
candidates and left merely-narrowed cells at their current values in the probe. The contradiction I
verified then depended on facts the clause never mentioned, which makes the clause invalid. Line logic
reasons from partial knowledge, so **every** cell narrowed since the root is a candidate.

**Expected:** the raw reason is 13.7–28.0 literals — *wider* than the decision clause on easy
puzzles. Minimized it is **2.7–5.6**, and it is bounded by lane length rather than trail depth, so it
does not grow as the search deepens. Cost: ~25 lane scrubs per conflict, measured at 0.4–3%.

### [ ] 3.3 The colour-puzzle question: measure it, don't assume

A `Nogood` literal is `(cell, colour)` meaning "this cell **is** that colour". A merely-narrowed cell
needs "this cell is one of {...}", which the vocabulary cannot say, and strengthening it to a single
colour is sound but nearly worthless (it rules out one case and lets the others escape).

**Measure how often it comes up** before deciding to extend anything. Five of six puzzles I checked:
**zero** merely-narrowed literals in any minimized reason. One colour puzzle (`color-02257`): 121 of
1158 literals, affecting 74 of 369 reasons (20%). So: skip those conflicts and let the guess-based
clause handle them alone. No new vocabulary needed.

### [x] 3.4 Learn the lane reason as an extra nogood

Add it *alongside* the guess-based clause, which still decides the backjump.

**First, count whether it could decide the backjump — it cannot.** A clause asserts only if exactly
one literal sits at the deepest level it mentions. Measure it: **0 of every reason**, on every puzzle,
because the contradiction was just created by the current guess's propagation narrowing several cells
in that lane at once. The deepest level it mentions always equals the current trail depth, so there
are no untouched levels to skip either. This is the thing to understand before Phase 4.

**Result:** 18 → 21 of 31 puzzles solved, `color-05193` −70%. So narrow cell-based clauses cut the
search through *propagation alone*, with the backjump unchanged. Superseded by Phase 4 but it works
standalone.

---

## Phase 4 — 1UIP

Now make the clause asserting, which is what buys the deep backjump.

### [x] 4.1 Put a reason on every trail entry

Change the trail element to carry `Decision | Lane(LaneIdx) | Nogood(usize)`. Line logic sets
`Lane`, guesses set `Decision`, nogood deductions set `Nogood`. About 15 edit sites.

### [x] 4.2 The resolution loop

Start from the minimized reason for the contradiction. While more than one literal sits at the
deepest level: take the **most recently assigned** of them, look up the reason it was assigned, and
replace it with that reason's literals. Stop when exactly one literal remains at the deepest level —
that is the first unique implication point. Backjump to the **second-highest** level in the clause;
the clause is then unit and forces the remaining literal.

Reason lookup by kind:
- `Lane(l)`: the same deletion minimization, on the lane **as it stood before that trail entry**, with
  the opposite value *pinned* and never dropped. "Why isn't this cell that colour" is just "explain
  the contradiction you get when you say it is".
- `Nogood(i)`: that nogood's other literals.
- `Decision`: no reason. Bail to the replay.

**Two guards you need:**
- Reject a reason naming anything settled **no earlier** than what it explains. Otherwise resolution
  can walk in a circle and the clause stops being a consequence of the trail's prefix.
- Bound the loop, and fall back to the old replay whenever analysis can't finish. Fallbacks were
  1–8% of conflicts for me.

**Do not reconstruct the whole grid** to get the historical state — rebuild just the lane's ~30 cells
by walking the trail backwards. Cloning a 3300-cell grid per reason lookup was most of what the next
step cost.

**Expected:** **18 → 26 of 31 solved**, eight newly solved, none lost, no disagreements with pbnsolve.
Replay drops from 50–96% of line logic to 1–22%; "deriving" from 20–35% of the clock to 0–3%. Big
wins on hard puzzles (−42% to −87%), small losses on easy ones (+29% to +66%) where there are too few
conflicts to amortize.

**Sanity check to expect:** 2.5–3.4 literals at the deepest level per conflict, so the loop is short
in principle — but it takes 33–135 resolutions in practice, because each reason introduces fresh
literals at the current level. Clauses end up 24–98 literals wide, which is why Phase 5 exists.

---

## Phase 5 — clause minimization

### [ ] 5.1 Recursive self-subsuming resolution

The clause is a conjunction known to be impossible. If the rest of it implies literal `C`, then `C`
added nothing, and the shorter clause rules out strictly more. Test `C` via its reason: `C` is
redundant if every literal of its reason is already in the clause, or is at level 0, or is *itself*
recursively implied. Memoize, cap the depth, give it a budget, and treat "out of budget" as
"not implied" — that only leaves a wider clause, never an unsound one. Never remove the one literal
at the deepest level; it is what makes the clause assert. Removals compose, so greedy sequential
removal is sound.

### [ ] 5.2 Persist the reason memo across conflicts — this is the whole ballgame

A reason depends only on the trail up to and including its own entry, so it stays valid until a
backjump cuts the trail back past it. Keep the memo on the search state and `retain(|entry| entry <
trail_idx)` on backjump. Successive conflicts share nearly all of their trail, so hit rates run
54–94%.

**Expected, with the memo:** clauses ~40% narrower, conflicts 20–45% fewer, **total benchmark time
−38%** — driven by the slowest puzzles (`color-02257` −81%, `color-02684` −70%, `webpbn-03541` −56%).
But it loses `webpbn-08098` and costs 5–120% on several quick ones, so leave it off by default.

**Expected, without the memo:** +15% and a net loss everywhere. Same code. If your numbers look like
this, the memo is what's missing.

---

## Things I looked at and would not bother repeating

- **Fixed-interval restarts.** They do *nothing*. The descent after a restart makes the **identical
  first guess**, because the picker's scores are read off a grid the restart put back exactly as it
  was, and the VSIDS drift and shuffle only break ties. Costs a little more picking and reaches fewer
  cells per guess. If you want restarts to work you need real diversification — a small probability of
  returning a random candidate, or simply restarting from scratch with a fresh seed, which the heavy
  tail in 0.1 says would pay well on some puzzles.

---

## Still untried, roughly by value

- [ ] **Point the fuzzer at conprop.** `solver_fuzzer` only drives line logic, so none of Phases 3–5
  has fuzz coverage. Given that both of my soundness bugs were caught by a 31-puzzle status
  comparison and nothing else, this is worth more than anything below.
- [ ] **Watched literals.** `propagate` walks the entire nogood database every round: 878 visits for
  32 deductions on one puzzle, 3,993,289 for 17,279 on another. Two watches per clause makes database
  size nearly free — and then clause *deletion* stops mattering, which is why I would do this instead
  of an expiry policy. (If you do delete: `nogoods_by_cell` holds indices into `nogoods`, so removal
  needs tombstones or remapping, and a clause that fired is the reason for a deduction still on the
  trail.)
- [ ] **Adaptive clause minimization** — switch it on once the trail is deep or conflicts are
  numerous, since that is exactly where it wins.
- [ ] **Cheaper reason lookup.** A real solver's reason is a pointer, not a re-derivation. Everything
  in Phases 4–5 is bottlenecked on `reason_at`.
- [ ] **Turn `one_uip` on by default** — after the fuzzer covers it.
- [ ] Five puzzles still time out in the standard set (all `multiple`): `webpbn-02712`,
  `color-02498`, `color-02984`, `color-03149`, `color-04445`.

---

## How to measure without fooling yourself

- **`cargo build --release` does not build `bench-pbnsolve`** — it needs `--features bench-pbnsolve`.
  I measured a stale binary and got +37%/+86% regressions where the real numbers were +4%/+5%.
- **Don't compare geometric means across different puzzle sets.** The figure "improved" from 0.33x to
  0.25x when the solver got strictly better, because the mean was suddenly averaged over eight
  additional hard puzzles that pbnsolve does in milliseconds. Compare *counts solved* and
  *per-puzzle times on the puzzles both configurations finish*.
- **Sweep seeds.** See 0.1.
- **Don't run other CPU work alongside a timing run.** I contended with my own benchmark once and had
  to downgrade that run's numbers to "what finishes".
- **Check `unique` vs `multiple` against pbnsolve on every change to clause learning.** It is the only
  soundness net these paths have.
