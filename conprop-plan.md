

# Task list
265abbb already has the engine and the bench modes; everything below is the uncommitted work.

## Foundations — do these first, or the picker numbers won't reproduce

* root_state — clone ll_state into the state just before the first guess; make_nogood forks it (self.root_state.clone()) instead of building SolveState::new(blank_grid). Never refresh it: a frozen root is only ever weaker, and anything learned since is in the nogoods. Also lets make_nogood drop its puzzle parameter.

* settle — line logic and the nogoods to a joint fixpoint on a replay grid, rescanning each nogood against that grid rather than trusting current_false_count (those counters describe ll_state's history, not the replay's). Use it at both sites in make_nogood — the "wrong on its own" early check as well as the reapply loop. ⚠️ I missed the first one once and it went unnoticed for a whole round. → conservative fallback 98.5% → 0%, nogoods roughly halved, 1.28×/1.47× faster.

## Picker — this is where ten of the eleven puzzles came from

* Colour preference (neighbors-flip) — same cell choice as Neighbors; break the colour tie away from what settled neighbours hold. Weight 0.001 × (count of neighbours already that colour), small enough to never override which cell is chosen. → 18 → 19, ~1.7× faster.
* RNG tie-break — splitmix64 over (seed, cell, colour), weight 0.0001. Not a live stream: the same candidate must get the same jitter all solve, or the ordering flaps. → 19 → 21.
* Activity — per-(cell, colour) score, +1.0 for every literal of each learned nogood, ×0.95 every 16 conflicts, normalised by a running maximum, weight 1.5 (in settled-neighbour units, so it can override cell choice). → 21 → 24, beating bt_solve's 23.

## Don't rebuild these — measured negative

* Front peel (binary-search the nogood's front): genuinely shrinks nogoods and the search, but costs more in replays than it saves. 19/31 at 1.9× slower.
* Restarts (Luby, guesses or conflicts): no benefit at either decay rate. Stateless pickers gave them nowhere to go; with activity they measurably change the search but still don't pay.
* Fast decay (0.95 per conflict, MiniSat's rate): 24 → 22. Your 16 was right.
* Lookahead: 20/31 — good, but beaten by 5 and far more expensive. (Human note: this is sort of a lie; worth looking into, and it might be *improved* with better guessing)

Instrumentation worth keeping: the --trace counter line and --conprop-picker on the bench. If you keep one metric, keep conflict depth — every intervention that helped pushed it down, and it predicted the others.

## How the picker system wants to work
Your instinct is right, and the evidence is that the trait was a ratchet: every improvement needed something it didn't have. The seed became an extra parameter, activity another, and lookahead couldn't be a GuessPicker at all — it lives in ConpropState with pick_guess reaching unreachable!().

Two distinct kinds of statefulness, and it's worth separating them:

* Memory — activity, which must survive guesses and backjumps and be updated on conflicts. Currently forced into ConpropState because GuessPicker::new runs per node.
* Incrementality — Neighbors, NeighborsFlip and Disagreement all rebuild their entire table on every guess: solidity and neighbour-colour counts over every cell, per-lane probabilities over every lane. That's O(cells × colours) per guess, and we make ~30,000 guesses on a 6,500-cell puzzle. The trail already records exactly which cells moved, so this is pure waste.

Both point the same way: build the picker once per solve and notify it, rather than constructing it per node from the grid.

```
trait Picker {
    fn learned(&mut self, cell: usize, before: Cell, after: Cell);
    fn unwound(&mut self, cell: usize, before: Cell, after: Cell);
    fn conflict(&mut self, nogood: &Nogood);
    fn restarted(&mut self);
    fn pick(&mut self, state: &ConpropState, probe: &mut Probe<'_>) -> Option<(usize, Color)>;
}
```
On making the state public to it: yes for reading — &ConpropState is what every picker I wrote actually wanted. But I'd stop short of &mut. Probing needs to fork and propagate, not to mutate the real trail, and handing over &mut ConpropState means a picker bug can corrupt the counters silently — which is exactly the class of bug that cost us the most time this session. A narrow Probe handle wrapping root_state.clone() + settle gives lookahead everything it needs while keeping the trail off-limits.

----

End metrics:

```
31 puzzles, 1 rep(s), pbnsolve (its default algorithms), timeout 30s
puzzle                   cells     loom sec      pbn sec    ratio  loom left  skims/scrubs  loom           pbn
webpbn-00023.xml           110     0.000561     0.000745    1.33x          0       402/149  unique         unique
webpbn-00027.xml           621     0.000355     0.000402    1.13x          0        225/43  unique         unique
webpbn-00065.xml          1360     0.004222     0.002793    0.66x          0      1391/466  unique         unique
webpbn-00436.xml          1400     0.232071     0.015303    0.07x          0   50210/22267  unique         unique
webpbn-00803.xml          2250     0.601597     0.081701    0.14x          0   71258/54574  unique         unique
webpbn-01611.xml          3300     0.008859     0.002891    0.33x          0      2765/650  unique         unique
webpbn-01694.xml          2250     0.231748     0.007411    0.03x          0   36995/16634  unique         unique
webpbn-02040.xml          3300     0.345752     0.166551    0.48x          0   24059/11966  unique         unique
webpbn-02413.xml           400     0.000952     0.000883    0.93x          0       548/153  unique         unique
webpbn-02556.xml          2925     0.010487     0.086235    8.22x        219      1057/657  multiple       multiple
webpbn-02712.xml          2209            -     1.361124        -          -             -  TIMEOUT(wall)  multiple
webpbn-03541.xml          3000     0.332989     0.008733    0.03x          0   25953/14566  unique         unique
webpbn-04645.xml          3500     0.038906     0.014683    0.38x          0     5736/1860  unique         unique
webpbn-06574.xml           625            -     0.699883        -          -             -  TIMEOUT(wall)  unique
webpbn-06739.xml          1600     2.747112     0.175014    0.06x        780  156275/69472  multiple       multiple
webpbn-08098.xml           361            -     2.182933        -          -             -  TIMEOUT(wall)  unique
webpbn-10810.xml          3600     0.031720     1.471782   46.40x        552     1615/1123  multiple       multiple
webpbn-color-01503.xml     625     0.034444     0.004172    0.12x          0     5666/2483  unique         unique
webpbn-color-02073.xml    1225     0.064086     0.008249    0.13x          0    15034/6558  unique         unique
webpbn-color-02257.xml    3300     0.053926     0.021932    0.41x          0    10176/4192  unique         unique
webpbn-color-02498.xml    3375            -     0.313292        -          -             -  TIMEOUT(wall)  multiple
webpbn-color-02684.xml    2592            -     1.316889        -          -             -  TIMEOUT(wall)  multiple
webpbn-color-02814.xml    2250     0.673296     0.017284    0.03x          0   64431/27572  unique         unique
webpbn-color-02817.xml    2250     1.134029     0.029153    0.03x          0 222894/109601  unique         unique
webpbn-color-02984.xml     625     0.023917     0.011919    0.50x        354     3217/1690  multiple       multiple
webpbn-color-03149.xml    1600     3.070359     0.131271    0.04x        971 225315/111080  multiple       multiple
webpbn-color-04364.xml    1600     4.424727     0.008548    0.00x          0  170946/56440  unique         unique
webpbn-color-04445.xml    6534            -     0.077427        -          -             -  TIMEOUT(wall)  multiple
webpbn-color-04809.xml    2000            -     0.694141        -          -             -  TIMEOUT(wall)  multiple
webpbn-color-04940.xml    2025     0.738724     0.024734    0.03x          0   65266/27680  unique         unique
webpbn-color-05193.xml     900     0.148435     0.281449    1.90x        633   19515/11117  multiple       multiple

24 of 31 puzzles fully solved by the backtracker
geometric mean over the 24 comparable puzzle(s): number-loom is 0.22x pbnsolve's speed
```

That's your target — --mode conprop --conprop-picker neighbors-flip, with ACTIVITY_WEIGHT = 1.5, ACTIVITY_DECAY = 0.95, ACTIVITY_DECAY_EVERY = 16, RESTART_UNIT = 0. I confirmed the current binary reproduces it to within timing noise.

Cheap checks that you've got it right, in order of how sharply they discriminate:

24 solved, 7 timeouts. The timeouts should be exactly webpbn-02712, -06574, -08098, webpbn-color-02498, -02684, -04445, -04809.
The loom left column. Six puzzles come back ambiguous with specific counts — 219, 780, 552, 354, 971, 633. Those are the root-knowledge snapshot working; if they're 0 you've got the "report the polluted root" bug back, and if they differ the snapshot is being taken at the wrong moment.
Three that only this config gets: webpbn-06739, webpbn-color-02817, webpbn-color-03149 are the activity heuristic's contribution. If you're at 21 and missing those, activity isn't wired in.
webpbn-10810 at 46.4× is the standout — a puzzle bt_solve times out on and pbnsolve takes 1.47s over. If that one's fast, propagation and nogoods are healthy.
Geomean 0.22×. Aggregate speed is the least sensitive check — the coverage and loom left columns will catch mistakes long before this moves.


## Metrics worth rebuilding
Keep these four:

* Conflict depth histogram — the master variable. Every intervention that helped pushed it down, and it explained the other metrics rather than merely correlating with them. If you rebuild one thing, this.
* Nogood size histogram — tightly coupled to depth, but it's what tells you whether minimization is working, and it reads differently (size is an output, depth is a cause).
* conservative count — cheapest and highest-value-per-line of anything I added. It caught a 98.5% silent failure that nothing else would have surfaced: make_nogood appeared to work, produced sound nogoods, and had simply stopped minimizing. Treat it as a health check, not a curiosity — if it drifts above ~0 you've broken settle. (Human note: make it a panic, not a metric)
* Guess count and solution depth — trivial, and the denominator for everything else.

Skip these:

* never-fired — came back exactly 0% in every configuration measured. A clean negative: every nogood we learn gets used, so there's nothing for clause deletion to do. Worth rebuilding only if you add deletion.
* Backjump distance — earned its keep once, by revealing the search was near-chronological (1.5–2.3 levels), but it never moved much afterwards and never drove a decision.
* trimmed — only meaningful while actively working on minimization.
Fired histogram — mostly restates the size histogram. The one insight (short nogoods fire 3–4× more often per nogood) was interesting but didn't change anything.
* Guess accuracy vs the goal solution — it answered your question, but equivocally: 20 points for neighbors, nothing for disagreement, and half of the gap explained by colour base rates. It also needs the goal plumbed through the bench. I'd not rebuild it as a standing metric.

One I'd add that I didn't have: mean conflict depth as a single scalar. The histogram is what you want when diagnosing, but a one-number summary makes A/B comparisons between picker tweaks immediate without eyeballing eleven buckets — and given that depth turned out to be upstream of everything, it's close to a one-number quality score for a configuration.