`number-loom` is a GUI and command-line tool for developing or solving nonograms.

Use `cargo run` to open the GUI, `cargo run --help` for options (including CLI capabilities). `trunk serve` builds and serves the wasm


# Text that users will read

If you're an LLM, don't write the final wording of anything a user of Number Loom will
read. Instead, write "HUMANIZE: <draft>", and a human will replace it. Please report how many places remain.

This covers labels, tooltips, status messages, dialog text, the README and the changelog. It
doesn't cover code comments, documentation for developers (like this file), or plans.

# File tree
  examples/ - various puzzles for testing purposes (filenames are spoilers for "puzzles/")
    woven/ - frozen `WOVEN` share strings; see the README there before touching them
  puzzles/ - puzzles for human entertainment; filenames are oblique references to the solution
  web/ - installable PWA stuff: manifest, service worker, and icons
  src/
    gui.rs - the core of the GUI: app state, the undo stack, the sidebar, and `main_ui`
    gui/
      compact.rs - small-screen layout
      canvas.rs - drawing the picture, and the pointer hit test
      annotate.rs - the solver's scratch marks for counting out a line (undo never sees them)
      selection.rs - the lasso: drawing a loop, what it caught, and moving the catch around
      tools.rs - the other tools (pencil, line, flood fill)
      palette.rs - the palette editor and its number-key shortcuts
      resize.rs - growing and shrinking the picture (for triddlers, it's complex!)
      toolbar.rs - the controls across the top, and the dialogs they open
      auto_button.rs - a button to do something once, with a "do it every change" toggle.
      solver.rs - the solving view: clue rendering, line analysis, and the solve replay
      triddler.rs - special triangular grid drawing: rhombus clue gutters and the rosette
      triano.rs - special trianogram drawing: half-square "cap" colors and clue silhouettes
      gallery.rs - for chosing a puzzle to solve
      input_mode.rs - mouse or touch: whichever the user used last
      touch.rs - in touch mode, a finger steers a cursor on the grid, and the tools act there
      outline_text.rs - overengineered halos for text readability on arbitrary backgrounds
      session.rs - (web only) saving all the GUI state, so closing the app doesn't lose work.
      back_button.rs - (web only) the back button closes the topmost dialog, not the app
    import.rs - read files and extract clues from grids
    export.rs - write files 
    formats/
      char_grid.rs, html.rs, image.rs, olsak.rs, webpbn.rs, woven.rs - the file formats
      partial_cells.rs - the `?`/`[...]` notation for partly-known cells, shared by webpbn and WOVEN
    solve/ - the automatic solver (as opposed to gui/solver.rs, the interactive solving view)
      line_solve.rs - quick ("skim") and exhaustive ("scrub") line-logic implementation
      human_line_solver.rs - line-logic techniques that people use, for measuring difficulty
      grid_solve.rs - uses repeated line-logic to solve a puzzle
      conprop.rs - backtracking search based on constraint propagation
      conprop_picker.rs - how to decide where to guess
    geometry.rs - puzzle shapes: what cells exist, what lines they form, where they sit
    layout.rs - abstract drawing geometry (cell shapes, positions, grid lines, clue gutters)
    puzzle.rs - data structures
    user_settings.rs - preferences that outlive a session
    solver_fuzzer.rs - stress test for solver correctness
    ocr-clue-bot/ - `ocr-clues`, which reads clues out of a picture of a puzzle (has its own DEVELOPING.md)
    bin/bench-pbnsolve.rs - speed comparison against `pbnsolve` (see below)
    bin/eval-techniques.rs - compares "human" techniques
    bin/reddit-bot.rs - runs `ocr-clues` on new posts to r/nonograms (but doesn't reply yet)
  benches/ - benchmarks (currently quite limited)
    (see also src/bin/bench-pbnsolve.rs, below)

# Shapes

Puzzles come in two shapes — square, and triangular ("triddlers", made of ▲▼ cells with three
clue directions instead of two). `geometry.rs` separates *what cells and lanes exist* (`LaneMap`,
shape-agnostic, used by the solver) from *where they sit* (`Geometry<K>`, which adds coordinates
for `K = Square` or `Tri`, used by the editor). That split is why `grid_solve.rs` and
`line_solve.rs` don't need to know or care which shape they're solving. `Puzzle<C, K>` and
`Solution<K>` carry the shape at compile time where it's known statically; `DynPuzzle` /
`DynSolution` / `DynCoord` and the `with_puzzle!` / `with_solution!` macros handle it where it's
only known at runtime (a loaded file, the GUI's current document).

# Benchmarking against `pbnsolve`

`pbnsolve`, by Jan Wolter, is one of the fastest and most complete nonogram solvers. Wolter's
[Survey of Paint-by-Number Puzzle Solvers](http://webpbn.com/survey/) is extensive. `examples/wolter/` is that
survey's puzzle set, in the webpbn XML that `pbnsolve` reads natively — so the two solvers can be
pointed at exactly the same puzzles. Build `pbnsolve` from source, then:

```
cargo run --release --features bench-pbnsolve --bin bench-pbnsolve -- --pbnsolve /path/to/pbnsolve
```

`--mode backtrack` benchmarks the search (`solve/conprop.rs`) instead of line logic, with both
solvers proving the solution unique; `--mode first-solution` has both stop at the first solution
they find.


# "Human" line-solving techniques

`solve/human_line_solver.rs` has line solvers are intended to be closer to how humans solve 
puzzles. `eval-techniques` runs all of the line solvers on random partly-solved lines, and
reports a bunch of statistics (can you tell an LLM implemented that bit?)

```
cargo run --release --features eval-techniques --bin eval-techniques -- --examples 3
```

`--examples` shows lines where the techniques fell short of each other, which is a good place to
look for the next technique to write. A new technique only needs an entry in `Technique`: that
gets it fuzzed (by `solver_fuzzer.rs`), and evaluated both alone and as part of `human*` (all of
the techniques in `human_line_solver.rs`, taking turns until they stop getting anywhere).

`Technique::tranches()` groups the techniques by difficulty, easiest first. The solve view's
"Analyze Lines" grades each line by the easiest thing that gets anywhere on it: "A" for the first
tranche, and so on through the tranches, then one more letter for a skim, and another for a scrub
(`LineDifficulty` in `grid_solve.rs`).

`--tranches` tries out a different grouping (like `contig-skim/done-by-size,extend/find-a-home`),
and reports how far each tranche gets on top of the easier ones, and how much it would miss
without each of its techniques.

`rule_out_finished_colors` is a "freebie" that's applied before anything is measured.

A clue is "fixed" when `skim_to_find_fixed_clues` says so (which is also what crosses it off in
the GUI). The techniques that count blocks leave fixed clues alone.

# OCR

`ocr-clues` reads the clues out of a picture of a puzzle. It has its own notes, in
`src/ocr-clue-bot/DEVELOPING.md`.

`reddit-bot` watches r/nonograms, and runs `ocr-clues` on the first picture in each new post. For
now, it only reads: what it would reply is saved, along with the picture, what `ocr-clues` said,
and its debug image, in a directory for each post with a picture. `log.txt` lists each picture,
with one line on how it went. It only drafts a reply if the clues make sense: at least four rows
and columns with clues, which don't contradict each other. The reply is the advice from
`ocr-clues --message` (see `guidance.rs`), signed with the Reddit username in `REDDIT_OPERATOR`
(without it, there's no sign-off). Reddit only answers API requests with OAuth,
so it needs `REDDIT_CLIENT_ID` and `REDDIT_CLIENT_SECRET` from an app registered at
https://www.reddit.com/prefs/apps (it logs in as the app, not as a user):

```
cargo build --release --features ocr --bins
target/release/reddit-bot bot/          # --once to check once and stop
```

`--listing` reads a listing saved from `/r/nonograms/new.json` instead, for trying it out without
credentials `--one-image` reads a single picture (a file or a URL), prints the reply it
would get (if any), and keeps the rest in a directory named after the picture. The first time it runs, every post in the listing it gets (the newest 25) counts as
new.

# The `WOVEN` format

The `WOVEN` format is JSON (generated by `serde`), compressed with Brotoli, encoded as Base-64, and then surrounded by "WOVEN-" and "-". The golden tests (see examples/woven/README.md) are intended to ensure backwards-compatibility. If a data structure that gets serialized needs to change, its current version can be copied to "woven.rs" to keep the format the same. If all else fails, it's possible to bump the version by adding a new entry to `enum WovenDocument`.

# Releasing a new version
(https://paulstansifer.github.io/number-loom/ is built automatically from `main` every push)

1. Check the new section of `CHANGELOG.md`; make sure the date is right, and see if any changes 
   are missing.
1. Check the `README.md` and `cargo run -- --help` to see if anything has gotten out-of-date.
2. Bump `version` in `Cargo.toml`, `cargo run -- --version` should print the new number.
3. Run the checks:

   ```
   cargo test
   cargo fmt --check
   cargo clippy --all-targets                             # not usually clean, but we can dream
   cargo check --target wasm32-unknown-unknown            # what itch.io serves
   cargo check --all-targets --features bench-pbnsolve,eval-techniques,ocr  # off by default, try them!
   ```

4. Check `cargo package --list` for stray files that should be ignored.
5. `trunk build --release`, then `(cd dist; zip -r dist.zip .)`, then upload `dist/dist.zip` to
   https://itch.io/game/edit/3651269
   And maybe check that the gallery works right from there — that's the one feature that needs
   the network, and itch.io is a different origin from where the puzzles live (see below).
6. `cargo publish`.
7. `VERSION=????`  # Bare, like "1.2.3"
8.  `git tag -a $VERSION -m "Version $VERSION"`, and `git push --tags`.

# The puzzle library

Every push to `main` triggers `.github/workflows/pages.yml`, which rebuilds the web app and
`puzzles.zip`, and publishes them to GitHub Pages. The Pages build comes from `pages.html` (the
app in a frame, next to some explanatory text) rather than `index.html` (the app filling the
whole window, which is what itch.io gets); try it locally with `trunk serve pages.html`. The archive is at
https://paulstansifer.github.io/number-loom/puzzles.zip; it probably takes around 15 minutes for puzzles to show up.

It has to be Pages and not a release asset in order to get
`Access-Control-Allow-Origin: *`. 

# Phones

The web build is an installable PWA (see `web/`), with a layout of its own for small screens.

## Two layouts

Below 600 points wide (`ScreenLayout::of`), the app uses the compact layout in `gui/compact.rs`:
a bar across the top, the tools and palette across the bottom, everything else in a drawer, and
full-screen dialogs. egui on the web uses one point per CSS pixel, so that's 600 CSS pixels, and
it's picked every frame, so narrowing a desktop browser window switches layouts too. Both layouts
are built from the same pieces (`edit_sidebar` and `SolveGui::sidebar` are the desktop's
arrangement of them), so a fix to one of those reaches both. Everything in the compact layout
that can be tapped is at least 48 points.

`pages.html` has breakpoints of its own, kept clear of the app's so that the page's framing never
squeezes the app into the compact layout: below 700px it gives the app the whole window, and below
1100px it puts the explanatory text under the app instead of beside it.

## Touch

The input mode (`gui/input_mode.rs`) follows whatever was used last, independently of the
layout: a touch means touch mode, and a pointer moving with nothing pressed means a mouse. In touch
mode, the canvas doesn't care where the finger is, only how it moves (`gui/touch.rs`):

- **Drag** anywhere on the canvas: moves a cursor on the grid, like a trackpad. It snaps to a
  cell when the finger lifts.
- **Tap:** a click of the current tool at the cursor.
- **Hold still, then drag:** the tool goes down at the cursor and is dragged along with it.
- **Two fingers:** pan and zoom. A second finger arriving cancels whatever the first one started.

Ruling a cell out by touch means picking the background from the palette. The numbers to tune
(`TAP_SLOP`, `HOLD_DELAY`, `CURSOR_GAIN`) are at the top of `touch.rs`.

## Testing on a phone

`trunk serve --address 0.0.0.0`, then open `http://<this computer's address>:8080` on the phone,
over the same network. That's plain HTTP, which browsers don't count as secure, so there's no
service worker there: no installing, and no working offline. Either test those against GitHub
Pages, or (on Android, with USB debugging on) run `adb reverse tcp:8080 tcp:8080` and open
`http://localhost:8080` on the phone instead, which does count as secure.

To debug, connect an Android phone by USB with USB debugging on, and open `chrome://inspect` in
Chrome on the computer: each tab on the phone (and an installed PWA) shows up there, with the
console and a live view of the screen. The service worker serves cached copies network-first, but
`#dev` at the end of the address skips registering it.

Without a phone: Chrome's DevTools device emulation sends touch events (pointer and touch events
both, as a phone does), and so does headless Chrome driven over the DevTools protocol
(`Emulation.setTouchEmulationEnabled`, then `Input.dispatchTouchEvent`). `tests/gui.rs` has the
compact layout's tests, which run at 390×844.
