# Developing `number-loom`

This is a guide for anyone — human or LLM — working on the `number-loom`
codebase. It covers how the project is put together, how to build and test
it, and where to look when making a change.

## What this project is

`number-loom` is a Rust tool (library + binary) for constructing and solving
nonograms ("Paint By Numbers" / "Griddlers" puzzles), including a rare
triangular-cell variant called "Trianograms". It has three faces:

* A CLI for converting between puzzle file formats and running the solver.
* A GUI (built with [`egui`](https://github.com/emilk/egui)/`eframe`) for
  editing, solving, disambiguating, and test-solving puzzles.
* A WASM build of the same GUI, published at
  https://paul-stansifer.itch.io/number-loom, built with
  [`trunk`](https://trunkrs.dev/).

The GUI and CLI share all core logic (formats, solver) through the
`number_loom` library crate (`src/lib.rs`); `src/bin/number-loom.rs` is a
thin CLI wrapper around it.

## Building and running

Requires a recent stable Rust toolchain (install via [rustup](https://rustup.rs/)).

```sh
# Build and run the CLI/GUI natively.
cargo build
cargo run -- examples/png/keys.png --gui   # open the GUI on a puzzle
cargo run -- examples/png/hair_dryer.png   # solve from the command line
```

With no arguments, `cargo run` opens the GUI editor on a blank puzzle.

### Running it as a web app

The web build uses `trunk` (`cargo install trunk`) and targets
`wasm32-unknown-unknown` (`rustup target add wasm32-unknown-unknown`).
`.cargo/config.toml` sets the `getrandom_backend="wasm_js"` cfg flag needed
by that target.

```sh
trunk serve   # serves the WASM build at http://127.0.0.1:8080, live-reloading
trunk build   # produces a release build in dist/ (see Trunk.toml)
```

`index.html` is the trunk entry point; it loads the `number-loom` binary as
WASM into a full-page `<canvas>`.

## Tests

```sh
cargo test
```

Notable tests:

* `src/lib.rs`'s `solve_examples` test runs the solver over every puzzle in
  `examples/png/` and asserts on the exact skim/scrub counts and unsolved
  cell counts reported for each. This is a **regression/consistency test**:
  if you change the solver's behavior (even to fix a bug or make it smarter),
  you will likely need to update the expected counts in that test to match
  the new, intentional behavior. Don't "fix" it by reverting your change
  without checking whether the new numbers make sense.
* `tests/gui.rs` uses [`egui_kittest`](https://docs.rs/egui_kittest) to
  drive the actual GUI (clicking buttons, sending pointer events) and assert
  on resulting state — e.g. that the solve/undo/redo buttons and the palette
  editor work. Prefer this style of test over hand-inspecting GUI code when
  you touch `gui.rs` or `gui_solver.rs`.
* `src/solver_fuzzer.rs` is a separate integration test binary (`solver-fuzzer`
  in `Cargo.toml`) that generates random puzzles to exercise the solver more
  broadly than the fixed example set.

There's also a Playwright script at
`jules-scratch/verification/verify_solver_sidebar.py` that screenshots the
running web app (`trunk serve` must already be running on `127.0.0.1:8080`)
for visual sanity-checking; it's a manual verification aid, not part of
`cargo test`.

## Benchmarks

```sh
cargo bench
```

`benches/benchmark.rs` uses `criterion` to benchmark the solver.

## Code layout

```
src/
  lib.rs           Library entry point; declares the modules below.
  puzzle.rs         Core data model: Puzzle/Document/Solution, colors, clues
                     (the `Clue` trait, implemented by `Nono` and `Triano`),
                     palettes, and the "dynamic puzzle" (`DynPuzzle`) wrapper
                     that lets code work over either clue style.
  import.rs         Loading puzzles from files (images, webpbn XML, olsak,
                     char-grid, woven) into a `Document`.
  export.rs         Writing puzzles back out to those formats, plus image
                     and HTML export.
  formats/
    webpbn.rs        webpbn's XML format (.xml / .pbn).
    olsak.rs         The Olšák solver's format (.g).
    woven.rs         `.woven`, Number Loom's own compact text format.
  line_solve.rs     The line-logic solver core: "skim" (push clues to each
                     side, check for overlap) and "scrub" (enumerate all
                     placements of a line's clues, intersect) heuristics.
  grid_solve.rs     Drives line_solve.rs over a whole grid until fixpoint;
                     reports solve counts/difficulty; also the disambiguator
                     (`disambig_candidates`), which finds single-cell edits
                     that reduce ambiguity.
  gui.rs            The main GUI: canvas editing, tools (pencil, flood fill,
                     orthographic line), undo/redo, palette editing, metadata
                     editing, mode switching.
  gui_solver.rs     "Puzzle mode": the test-solving UI (painting a solution
                     against the clues, error/progress indicators).
  gui_gallery.rs    The example-puzzle library/gallery UI.
  user_settings.rs  Cross-platform persistent key/value settings store
                     (native: `preferences` crate; wasm: `gloo-storage`
                     `LocalStorage`).
  bin/number-loom.rs CLI argument parsing (`clap`) and dispatch to
                     import/export/solve/gui.
```

Other top-level directories:

* `examples/png/` — a fixed set of example puzzles (as images) used by the
  `solve_examples` regression test and by the in-app example gallery. Names
  here spoil the puzzle's solution, so keep that in mind before adding one.
* `puzzles/` — the "real" curated puzzle library shipped/archived for users
  (zipped and attached to a GitHub release by
  `.github/workflows/puzzle_archive.yml` whenever files here change on
  `main`).
* `benches/` — `criterion` benchmarks.

## Solver concepts worth knowing

The solver is a **line-logic solver only** (no backtracking/guessing). For
each line (row or column) it can run two increasingly thorough passes:

* **Skim** — shove all of a line's clues as far as possible to one side,
  then the other, and see where the two placements are forced to agree.
  Cheap, but doesn't extract everything.
* **Scrub** — enumerate every possible placement of every clue in the line
  and intersect across all of them. This gets everything derivable from a
  single line in isolation.

`grid_solve.rs` repeatedly runs skim/scrub across all lines until nothing
changes, tracking per-cell *possible* colors (not just "known"/"unknown") —
this mirrors how a human solver's ad-hoc reasoning tends to work when
glancing at both lines through a cell. The skim/scrub counts reported after
solving are a rough proxy for a puzzle's difficulty (see the README's
"Solver" section for interpreting them).

The **disambiguator** (`grid_solve::disambig_candidates`) tries every
possible single-cell change to a solved puzzle and re-solves, to find
one-cell edits that would make an under-constrained puzzle more (or fully)
solvable. It caches intermediate solver state to make this tractable.

## Conventions and gotchas

* **Native vs. WASM code paths**: several modules (`user_settings.rs`, parts
  of `Cargo.toml`'s dependency list) are conditionally compiled per target
  via `#[cfg(target_arch = "wasm32")]`. When changing platform-dependent
  code, check both `cargo build` and `cargo build --target wasm32-unknown-unknown`
  (or `trunk build`) still work.
* **`DynPuzzle` / the `Clue` trait**: puzzle logic is written generically
  over the `Clue` trait so that ordinary nonogram clues (`Nono`) and
  triangular-cap clues (`Triano`, for Trianograms) share almost all code.
  When adding puzzle-level features, prefer extending the trait/generic code
  in `puzzle.rs` over special-casing one clue type, unless the feature is
  genuinely specific to one variant (e.g. only `olsak` and `char-grid`
  support Trianograms today).
* **Format round-tripping**: not every format supports every feature (e.g.
  only `olsak`/`char-grid` support Trianograms; only `webpbn` currently
  imports/exports metadata). When adding a field to `Puzzle`/`Document`,
  check whether each format in `src/formats/` and `import.rs`/`export.rs`
  needs updating, and whether silently dropping the field on unsupported
  formats is acceptable or should warn (see `Document::quality_check` for
  the existing warning mechanism used by the CLI).
* **Changelog**: `CHANGELOG.md` is kept up to date by hand, with an
  "unreleased/future" section at the top for the next version. Add
  user-facing changes there under the current `## 0.x.y - future` heading.
* **`TODO.md`**: a lightweight backlog of known-wanted changes/ideas; check
  it for context before starting unrelated cleanup, and feel free to add to
  it instead of doing speculative work outside what was asked.

## Formatting and linting

Standard Rust tooling applies; there's no repo-specific `rustfmt.toml` or
clippy config, so defaults are used:

```sh
cargo fmt
cargo clippy
```
