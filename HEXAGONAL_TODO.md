# Hexagonal (triddler) nonograms — implementation checklist

Support for puzzles made of **triangular cells**, with **three** clue axes instead of two.
This is webpbn's `type="triddler"` (see `webpbn_tridder.md`).

## Decisions already made

* **Cells are triangles**, tiled ▲▼▲▼ per horizontal row, exactly as webpbn encodes them.
  (Not hexagonal cells — the name "hexagonal nonogram" refers to the usual hexagonal *outline*.)
* **Arbitrary outlines** are allowed, per webpbn: rows may bend, and some of the six clue-sets
  may be empty (sharp corners). So hexagons, trapezoids, and triangles are all representable.
* **Dedicated coordinate storage**, not a rectangular bounding box with an `OUTSIDE` sentinel.
* Geometry is **orthogonal to `ClueStyle`**: add a separate `Geometry` axis rather than a
  `ClueStyle::Hex` variant, so future clue formats compose with future geometries. Triangular +
  `Triano` is rejected at construction/load time, but the type system doesn't forbid it.
* **The Woven format may be broken**; no files exist in the wild. No migration path needed.

## Design, settled by investigation

All of the following was **verified mechanically** against the worked example in
`webpbn_tridder.md` (scratch program reproduced the doc's `/ABCDE\` `/FGHIJK/` `\LMNOP/` rows and
its exact six clue-set line counts).

### Coordinates: a cell is `(a, b, c)` with `a - b + c ∈ {-1, 0}`

`-1` means ▲, `0` means ▼. The three clue-line families are exactly "`a` constant",
"`b` constant", "`c` constant" — so lane extraction is symmetric and trivial.

* `a` = horizontal row index; `b` = `/` line index; `c` = `\` line index.
* Every cell lies in exactly one line of each family (checked exhaustively).
* Neighbour steps (each is its own inverse family):

  | direction | from ▲ `(a,b,c)` | from ▼ `(a,b,c)` |
  |---|---|---|
  | right (along a row) | `(a, b, c+1)` | `(a, b+1, c)` |
  | left | `(a, b-1, c)` | `(a, b, c-1)` |
  | up-right (along a `/` line) | `(a, b, c+1)` | `(a-1, b, c)` |
  | down-left | `(a+1, b, c)` | `(a, b, c-1)` |
  | down-right (along a `\` line) | `(a+1, b, c)` | `(a, b+1, c)` |
  | up-left | `(a, b-1, c)` | `(a-1, b, c)` |

* **Caveat the solver must respect:** two lanes from different families can intersect in **two**
  cells, not one. (▲ and the ▼ to its right share both a row and a `/` line.) The square-grid
  assumption that lane *i* meets this lane at position *i* is doubly wrong — see Phase 2.
* Rendering: `(a, b, c)` → `(row, p)` where `p` is a horizontal half-unit and the triangle spans
  `[p, p+2]`: `p = 2b - a` for ▲, `p = 2b + 1 - a` for ▼.

### Outline: six bounds — which also solves resizing

A triddler outline is exactly `a ∈ [a₀,a₁]`, `b ∈ [b₀,b₁]`, `c ∈ [c₀,c₁]`. Every such box is a
valid puzzle and every valid puzzle is such a box. (Confirmed: the doc's 16-cell example is
precisely `a∈[0,2], b∈[1,3], c∈[-1,2]`.) This is forced — clues only make sense if every line is
contiguous, which makes the region convex, which makes it an intersection of six half-planes.

**Resizing is therefore easy, and easier than the square case: it is ±1 on one of six integers.**
No closure constraints, no shape validation, nothing can go out of sync. Verified by nudging all
six bounds in both directions: every result is a valid puzzle and the clue sets redistribute
themselves. A regular hexagon of side *n* is `a∈[0,2n-1], b∈[0,2n-1], c∈[-n,n-1]` (6n² cells).
Sharp corners fall out for free — a clue set is simply empty when its bound is not binding, which
is exactly what webpbn's "some clue-sets may be empty" means.

*Do not back out of resizing.* The only real cost is UI: six grow/shrink controls instead of four.

### webpbn's six clue sets = three families split at a bend

Which set a line belongs to is determined by **which bound stops the line at its clued end**:

| family | clued end | bound hit | clue set |
|---|---|---|---|
| `a` (rows) | left | `b₀` | `topleft` |
| `a` (rows) | left | `c₀` | `bottomleft` |
| `b` (`/`) | top | `a₀` | `top` |
| `b` (`/`) | top | `c₁` | `topright` |
| `c` (`\`) | bottom | `a₁` | `bottom` |
| `c` (`\`) | bottom | `b₁` | `bottomright` |

On a corner cell where both bounds bind, the first row of the table wins (the doc's example
requires this: its middle row is a corner and is filed under `topleft`).

* Lines are listed in **increasing index order**, and the two sets of a family concatenate in that
  order (`topleft` then `bottomleft`, etc.). Confirmed for rows by clue `2,3`, which needs ≥6 cells
  and so can only be the 6-cell middle row; confirmed for `\` lines by clue `2,1`, which needs ≥4
  cells and so cannot be the 3-cell line.
* The bend position never needs to be recovered separately — it is implied by the six bounds.
* **Reading direction within a line — SETTLED**, without needing a file from webpbn. Feeding the
  doc's clue values through the solver decides it: of the eight combinations of per-family
  direction, exactly one is even consistent, and under that one the example solves *completely*.

  | family | clue index 0 is at | relative to its label |
  |---|---|---|
  | rows | left end | at the label |
  | `/` lines | top end | at the label |
  | `\` lines | **top-left end** | **at the far end** |

  So `\` lines are the odd one out: they read top-left → bottom-right, *towards* the
  `bottom`/`bottomright` labels rather than away from them. My original guess had them backwards.
  `no_other_reading_direction_works` in `webpbn.rs` guards this — flipping any one family breaks
  the puzzle.

### Line solving is already geometry-agnostic

Everything in `line_solve.rs` takes `&[C]` plus an `ArrayViewMut1<Cell>`; the geometry lives
entirely in `grid_solve.rs`. So leave `line_solve.rs` **completely untouched**: gather a lane into
a temporary `Array1<Cell>`, solve, scatter back.

I claimed up front that the copy cost was free and needed no benchmark, on the grounds that
`op_or_cache` already gathers the lane for its cache key, `rescore` already makes an O(len) pass,
and `exhaust_line` is at least O(clues × slack²). **That reasoning was wrong**, and the benchmark
caught it. All three justifications assume scrubbing:

* `op_or_cache` is only on the **scrub** path, and only gathers when a cache is present —
  `solve()` passes `&mut None`. Skimming calls `skim_line` directly.
* `exhaust_line` never runs on a skim-only puzzle.
* `rescore` did make a pass, but over a *borrowed ndarray view*; it now copies first.

`fire_submarine` is skim-only (`skims: 161, scrubs: 0`), so it pays the gather with nothing to
amortise it against, and regressed 9.3% on the first attempt. Reusing a scratch buffer instead of
allocating an `Array1` per gather recovered most of it, and hoisting the invalidation `stale`
vector out of the solve loop recovered a bit more.

Measured on `main` (48b7cb6) before, and after the Phase 2 refactor:

| | before | after | change |
|---|---|---|---|
| `tedious_dust_40` (1549 skims, 328 scrubs) | 19.008 ms | 18.824 ms | **−1.0%** |
| `fire_sub` (161 skims, 0 scrubs) | 620.00 µs | 651.25 µs | **+5.0%** |

The scrub-heavy puzzle got slightly *faster* — the new invalidation is O(affected × families)
rather than O(lanes × affected). The skim-only puzzle is ~5% slower and that appears to be the
floor without special-casing square geometry: what remains is the lane copy itself plus a
bounds-check per cell that the old ndarray view iteration didn't have.

- [ ] Decide whether 5% on skim-only puzzles is worth reclaiming. A contiguous-lane fast path
      (square rows are contiguous in the flat buffer; columns are strided) would recover some of
      it at the cost of reintroducing geometry special-casing into the solve loop.

---

## Typed coordinates (done)

`Solution::at(x, y)` used to panic on a triddler; that class of bug is now a compile error.
`geometry.rs` splits into `LaneMap` (what the solver needs — no coordinates, no type parameter)
and `Geometry<K>` (coordinates and layout). `Puzzle<C, K>` and `Solution<K>` carry the kind;
`DynPuzzle` / `DynSolution` / `DynCoord` plus the `with_puzzle!` / `with_solution!` macros handle
the runtime boundary. See DEVELOPING.md.

Also landed here: `layout.rs` (abstract drawing geometry, nested row/cell iterators, O(1) hit
test, grid-line guides, clue-gutter anchors), `edge_neighbors` and per-family run lengths,
`K::SIDES` + `Geometry::resized`, and flat cell-indexed `solved_mask` / disambiguation reports.
Outlines are now stored raw and normalized only for equality, so resizing no longer renumbers
existing cells.

**Still to do:** the GUI is unchanged and remains square-only. It reaches its data through
`Document::square_solution_mut()`, which reports "the editor can't edit triddlers yet" through the
status bar instead of panicking. Wiring the iterators and hit test into the canvas, six-way clue
gutters, and the six-handle resizer are all still ahead.

## Phase 1 — Data model

**Done.** See `src/geometry.rs`. `Puzzle<C>` now holds a `Geometry` and a flat `lines:
Vec<Vec<C>>` indexed like `geometry.lanes()`, with `Puzzle::square`/`triangular` constructors and
`row_clues()`/`col_clues()` accessors. `PartialSolution` is a flat `Vec<Cell>` indexed by the
geometry's dense cell numbering (for square puzzles that is the same `y * width + x` layout the
old `Array2` used, so nothing observable changed).

`Solution` now carries a `Geometry` too, with a flat `cells: Vec<Color>` in place of the old
square-only `grid: Vec<Vec<Color>>`. Since cells may be `UNSOLVED`, that means an *ambiguous
triddler under development* is representable and round-trips through Woven. Square code uses
`at(x, y)`/`set(x, y, …)`; `to_columns`/`from_columns` bridge the few genuinely square-only places
(image export, char-grid, the row/column resizer).

Outlines are canonicalized (`Outline::normalized`) so that two descriptions of the same shape
compare equal — position carries no meaning, and letting it leak into equality broke round-trip
tests.

* [x] Add `Geometry { Square, Triangular }` in `puzzle.rs` (`clap::ValueEnum`, `Serialize`, `Copy`)
* [x] Add `TriCoord { a, b, c }` and `Outline` (the six bounds), with the neighbour-step table above
* [x] Dense storage: `Vec<Color>` in row-major `(a, then b)` order plus a per-row start offset,
      and a `TriCoord` ↔ flat-index pair. (Not a `HashMap` — this is the solver's inner loop.)
* [x] Precompute the three lane-index tables once per outline, and the reverse map
      cell → its three `(family, lane, position)` memberships
* [x] Add `geometry` to `Solution` and to `Puzzle<C>`; `Solution::x_size`/`y_size` and
      `Document::dimensions` need triangular-aware replacements
* [x] Replaced `Solution::grid` with a flat `cells: Vec<Color>` indexed by the geometry
* [ ] Reject `Geometry::Triangular` + `ClueStyle::Triano` wherever a `Solution` is constructed
* [ ] `Solution::blank_bw` gains a geometry/outline argument (or a `blank_hex` sibling) — still
      square-only, needed by the new-puzzle dialog in Phase 4
* [ ] `Solution::count_contiguous` — 3 axes, 6 directions (still square-only; used by the editor)
* [x] `Solution::quality_check` — now scales off cell count and lane count, which work for any shape

## Phase 2 — Solver

Done, and the solver demonstrably solves triddlers: see the `solves_*_triangular_*` tests in
`grid_solve.rs`. `solve_examples`, which pins exact skim/scrub counts for 34 puzzles, still passes
unchanged — good evidence the refactor is behaviour-preserving for square puzzles.

* [x] Introduce a lane abstraction: a lane is an ordered list of cell indices, produced from the
      geometry. Square geometry produces the existing rows and columns.
* [x] `PartialSolution` — currently `ndarray::Array2<Cell>`. Either generalize to a flat buffer +
      index map, or keep `Array2` for square and add a variant. This is the load-bearing change;
      it touches `grid_solve.rs`, `gui.rs`, `gui_solver.rs`, and `Solution::to_partial`.
* [x] `LaneState::row: bool` → an axis index (0..2 for triangular, 0..1 for square)
* [x] `get_grid_lane`/`get_mut_grid_lane` → gather/scatter through the lane's index list
* [x] **Intersection invalidation** in `solve_grid`: the current
      `other_lane.row != was_row && report.affected_cells.contains(&other_lane.index)` assumes
      lane *i* of the other axis meets this lane at position *i*, and that a pair of lanes meets in
      one cell. Both are false here (see the two-cell-intersection caveat above). Replace with a
      lookup through the precomputed cell → `(family, lane, position)` map: translate
      `report.affected_cells` (positions in *this* lane) to cell indices, then to the set of
      lanes to invalidate.
* [x] `settle_solution` and `analyze_lines` — iterate all axes, not just rows/cols
* [ ] `analyze_lines` returns `(Vec<LineStatus>, Vec<LineStatus>)`; make it per-axis. It now
      computes all families but discards the third, pending a GUI that can show it.
* [x] `LineCache<C>` keys on `(Vec<C>, Vec<u32>)` — works unchanged with gathered lanes ✅
* [x] `grid_to_solution` / `grid_to_solved_mask` — currently `grid.columns()`
* [ ] `disambig_candidates` — the `for x / for y` double loop becomes iteration over cells.
      No triangular-specific *logic* is needed; it is geometry-agnostic apart from the loop shape.
* [x] `--only-solve-color` needs nothing beyond the above; `filter_report_by_color` is per-lane
* [x] `solution_to_puzzle` in `import.rs` — now derives clues along every lane of any geometry

## Phase 3 — Import / export

webpbn triddler read and write are **done**, and a triddler now solves end-to-end from the CLI.
The outline is recovered from the six clue-set line counts alone (`Outline::from_clue_set_counts`),
which works because the six bounds have only four degrees of freedom — see `geometry.rs`.

Olsak triddler read and write are **also done**, validated against the real `tkocka.g` and
`vcely.g` from the Olsak distribution — both solve completely by line logic, and `tkocka.g` renders
as a recognizable cat.

`export::to_bytes` refuses to write a triangular puzzle in any format that can't express one
(image, HTML, char-grid, Woven-as-picture), rather than emitting nonsense.

### Olsak's triddler conventions, as determined

Olsak labels the hexagon's sides `A`..`F` counterclockwise from the upper left, giving
`A`=topleft, `B`=bottomleft, `C`=bottom, `D`=bottomright, `E`=topright, `F`=top. Two further facts
were *not* deducible from the documentation and were found by search (1024 readings fit
`tkocka.g`'s line lengths; exactly two solve it; one is the mirror of the other, and this is the
one matching the documented side diagram):

* `E` and `F` list their lines in **decreasing** lane order — they run the other way around the
  hexagon. `A`/`B` and `C`/`D` are in increasing order.
* `C` and `D` write the blocks **within** each line in the reverse of our order. This is what
  Olsak's warning about columns that "begin at the bottom of hexagonal ... read from underneath
  upstairs" means.
* Blank lines inside a data group are **significant** ("no block in this row/column") and must not
  be trimmed, including trailing ones. `tkocka.g` relies on this: three of its six groups end in a
  meaningful blank line, and trimming them yields an outline whose clues cannot be satisfied.
  (Olsak's own identities `E = A + B - D` and `F = C + D - A` hold under *both* readings, so they
  can't be used to distinguish.)

Note that round-tripping through Olsak renumbers colour indices and drops the human-readable
colour names (both pre-existing quirks of this format, not triddler-specific), so tests compare by
RGB rather than by `Color` index.

* [x] **webpbn read**: parse the six `<clues type=...>` sets and merge each pair into one family,
      using the bound-hit table above in reverse
* [ ] **webpbn read**: parse the `/ABCDE\` solution rows. The leading delimiter is `/` when the
      row's first cell is ▲ and `\` when it is ▼ (trailing is `\` for ▲, `/` for ▼); together with
      the row lengths this recovers the six bounds.
* [ ] Reconcile the two sources of outline info (clue-set line counts vs. solution row shapes) and
      error clearly when a file disagrees with itself. Reading currently uses the clue counts only;
      malformed counts are already rejected rather than guessed at.
* [x] **webpbn write**: split three families back into six clue-sets; emit `type="triddler"`.
      (The slanted solution rows still need `Solution`.)
* [x] **Woven**: `SerializableSolution` now stores `shape` + flat `cells`; breaking change, as agreed
* [x] **Olsak**: triddlers *are* supported (`#t`/`#T`, six data groups); read and write both done
      and validated against the real `tkocka.g` and `vcely.g`
* [ ] **HTML export** (`export.rs:62` `as_html`): three clue gutters at angles, or fall back to a
      plain textual clue listing for triangular puzzles
* [ ] **Char-grid** (`import.rs:135`, `export.rs:168`): decide whether to support triangular at all;
      if so, pick a convention (webpbn's `/`…`\` row delimiters are the obvious one)
* [x] **Image** (`import.rs:77`, `export.rs:137`): **out of scope** — we do not want to interpret
      images as triddlers. Reject with a clear error rather than misreading a PNG as a hex grid.
* [ ] `infer_format` and the `NonogramFormat` doc-comments (`CharGrid` currently claims to be the
      only Triano-capable format)

## Phase 4 — GUI: editing

* [ ] `EditGui::canvas` (`gui.rs:521`): triangular rendering. Note `triangle_shape` (`gui.rs:809`)
      already exists for Triano corner colors and may be partly reusable.
* [ ] Hit-testing: screen position → `TriCoord` (the current `canvas_pos.x as usize` won't do)
* [ ] Grid lines: three families instead of two; decide what the every-5th-line emphasis means here
* [ ] `cell_shape` (`gui.rs:830`): the solved/disambiguation overlays are drawn as rects
* [ ] `flood_fill` (`gui.rs:480`): triangular neighbours (each triangle has 3 edge-neighbours)
* [ ] `Tool::OrthographicLine`: three directions instead of two
* [ ] `Action::ChangeColor` keys on `(usize, usize)` — needs to key on `TriCoord`
* [ ] `resize` / `resizer` (`gui.rs:991`, `gui.rs:1039`): six grow/shrink controls, one per bound.
      The model side is trivial (±1 on an integer, always valid); the work is entirely in laying
      out six pairs of buttons around a hexagon instead of four around a rectangle, and in
      remapping existing cell contents when a bound moves.
* [ ] Guard against shrinking a bound until the puzzle is empty
* [ ] New-puzzle UI: choose geometry, and an outline (hexagon of side *n* is the sensible default)
* [ ] Disallow the Triano palette / corner-color UI when geometry is triangular

## Phase 5 — GUI: solving

* [ ] `draw_clues` (`gui_solver.rs:362`) and `draw_dyn_clues` (`gui_solver.rs:514`): six clue gutters
      at 0°/60°/120°, rather than the current left/top `Orientation` pair
* [ ] `Orientation` enum (`gui_solver.rs:309`) needs the new axes
* [ ] Layout/sizing maths for the whole solve view (a hexagon's bounding box is not the grid)
* [ ] Solve-mode canvas rendering + hit-testing (shares work with Phase 4)
* [ ] Line-status highlighting from `analyze_lines` across three axes

## Phase 6 — Testing

* [ ] Port the scratch verifier into real unit tests: the doc's 16-cell example reproduces rows
      `ABCDE`/`FGHIJK`/`LMNOP` and clue-set counts 2/1, 2/1, 2/2; each family covers every cell
      exactly once; `TriCoord` ↔ dense index round-trips; all six resize nudges stay valid
* [ ] Round-trip tests: webpbn triddler → `Document` → webpbn, and Woven
* [ ] Extend `solver_fuzzer.rs` to generate triangular puzzles
* [ ] Add a triangular puzzle to `benches/benchmark.rs`. Note the existing benches load from PNG,
      which triddlers won't support — they'll need a webpbn or Woven source file.
* [ ] Re-run `cargo bench` after the Phase 2 refactor to confirm no square-puzzle regression
* [ ] Add triangular examples under `examples/` and at least one puzzle under `puzzles/`
* [ ] `lib.rs:17` `solve_examples` should cover them
* [ ] `tests/gui.rs` — a smoke test for the triangular canvas

## Phase 7 — Docs

* [ ] `DEVELOPING.md`: describe the coordinate scheme and the lane abstraction (this is the part
      that will be non-obvious to a reader)
* [ ] `README.md`: mention triddler support
* [ ] `CHANGELOG.md`
* [ ] Note the Woven format break in the changelog

---

## Open questions

* [ ] What should the every-5th-line grid emphasis become on a triangular grid, if anything?
* [ ] Does `Corner` (the ▲/▼ half-square colors used by Triano) want to be renamed now that
      "triangle" means something else in this codebase?

### Resolved

* ~~Reading direction within a `/` or `\` line?~~ Settled by consistency search; see above.
  Still worth confirming against a real webpbn file when one is handy, but the evidence is strong —
  and Olsak's independent warning that its `\`-family blocks read "from underneath upstairs"
  corroborates that this family is the odd one out.
* ~~Does `Solution` need to represent ambiguous/partly-solved triddlers?~~ Done — it holds a
  `Geometry` and a flat cell array, and `UNSOLVED` cells survive a Woven round-trip.
* ~~Should outlines be resizable?~~ **Yes** — it is ±1 on one of six integers, and strictly simpler
  than the square case on the model side. Do not back out.
* ~~Does `--only-solve-color` / disambiguation need triangular-specific tuning?~~ No.
* ~~Is there a triangular analogue of PNG import?~~ Out of scope by decision.
