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
    puzzle.rs - data structures
    solver_fuzzer.rs - stress test for solver correctness
  benches/ - benchmarks (currently quite limited)