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

# Shapes: `LaneMap` vs `Geometry<K>`

Puzzles come in two shapes — square, and triangular ("triddlers", made of ▲▼ cells with three
clue directions instead of two). `geometry.rs` splits that into two pieces, and the split is the
one thing worth understanding before reading the solver or the editor:

* **`LaneMap`** is what the *solver* needs: how many cells there are, which ordered lists of cells
  form lanes, and which lanes each cell belongs to. It has no coordinates and no type parameter.
  Cells are numbered densely, `0..cell_count`.

* **`Geometry<K>`** is what the *editor* needs: it wraps a `LaneMap` and adds coordinates. `K` is
  a `GridKind` — either `Square` (coordinates are `(x, y)`) or `Tri` (coordinates are `TriCoord`,
  three numbers). It also carries O(1) coordinate lookup and the abstract layout below.

Because `grid_solve.rs` only ever touches a `LaneMap`, **solving is identical for every shape and
carries no type parameter**. `line_solve.rs` is one level further down still and doesn't even know
what a puzzle is. All the shape-awareness lives above them.

`Puzzle<C, K>` and `Solution<K>` carry the kind, so `sol[(3, 4)]` and `sol[TriCoord::new(a, b, c)]`
are both compile-time-checked, and a square-only operation such as `Solution::to_columns` simply
doesn't exist on a triddler. Where the shape is only known at runtime — a loaded file, the GUI's
current document — use `DynPuzzle` / `DynSolution` / `DynCoord`, and the `with_puzzle!` /
`with_solution!` macros to run one body against whichever kind is inside.

`DynPuzzle`'s three variants enumerate the *valid* combinations of clue style and shape; there is
no triangular Triano variant because that combination is meaningless.

## Triangular coordinates

A cell is `(a, b, c)` with `a - b + c` equal to `-1` (▲) or `0` (▼); the three clue families are
exactly "`a` constant" (rows), "`b` constant" (`/` lines) and "`c` constant" (`\` lines). An
outline is six bounds, so resizing is ±1 on one integer and can never produce an invalid shape.
Outlines are stored as given and normalized only for equality, which is what lets a resize leave
existing coordinates untouched.

Unlike a square grid, **two lanes from different families can share two cells** (a ▲ and the ▼ to
its right lie in the same row *and* the same `/` line), so nothing may assume lane *i* meets a
lane at position *i*.

## Abstract layout

`layout.rs` describes where to draw things in units where **every cell edge is 1.0 long**, with y
pointing down. `Geometry::rows()` gives a nested iterator over the picture carrying each cell's
dense index, coordinate, `CellShape` and origin; `Geometry::cell_at` turns a point back into a
coordinate in O(1). Neither knows about egui, so this all still builds for wasm and the CLI.
