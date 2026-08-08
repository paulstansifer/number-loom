`number-loom` is a GUI and command-line tool for developing or solving nonograms.

Use `cargo run` to open the GUI, `cargo run --help` for options (including CLI capabilities). `trunk serve` builds and serves the wasm


# File tree
  examples/ - various puzzles for testing purposes (filenames are spoilers for "puzzles/")
  puzzles/ - puzzles for human entertainment; filenames are oblique references to the solution
  src/
    gui{,_solver,_gallery}.rs - GUI implementation (for the editor, solver, and the puzzle-chooser)
    import.rs, export.rs - support for various file formats
    line_solve.rs - fast, complete line-logic implementation
    grid_solve.rs - fast, complete puzzle solving using line logic
    geometry.rs - puzzle shapes: what cells exist, what lines they form, where they sit
    layout.rs - abstract drawing geometry (cell shapes, positions, grid lines, clue gutters)
    puzzle.rs - data structures
    solver_fuzzer.rs - stress test for solver correctness
  benches/ - benchmarks (currently quite limited)

# Shapes

Puzzles come in two shapes — square, and triangular ("triddlers", made of ▲▼ cells with three
clue directions instead of two). `geometry.rs` separates *what cells and lanes exist* (`LaneMap`,
shape-agnostic, used by the solver) from *where they sit* (`Geometry<K>`, which adds coordinates
for `K = Square` or `Tri`, used by the editor). That split is why `grid_solve.rs` and
`line_solve.rs` don't need to know or care which shape they're solving. `Puzzle<C, K>` and
`Solution<K>` carry the shape at compile time where it's known statically; `DynPuzzle` /
`DynSolution` / `DynCoord` and the `with_puzzle!` / `with_solution!` macros handle it where it's
only known at runtime (a loaded file, the GUI's current document).

See the doc comments in `geometry.rs` for the coordinate scheme and lane details, and
`HEXAGONAL_TODO.md` for the design rationale and history.
