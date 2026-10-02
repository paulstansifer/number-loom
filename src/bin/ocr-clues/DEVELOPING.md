# `ocr-clues`

`ocr-clues` extracts the clues from a screenshot or photo of a (black-and-white, square) puzzle:

```
cargo run --release --features ocr --bin ocr-clues -- puzzle.webp puzzle.xml --debug-image debug.png
```

It's behind the `ocr` feature because it's big. The models need to be put in `~/.cache/ocrs/`
(where `ocrs-cli` also looks):

```
curl https://ocrs-models.s3-accelerate.amazonaws.com/text-detection.onnx -o ~/.cache/ocrs/text-detection.onnx
curl https://ocrs-models.s3-accelerate.amazonaws.com/text-recognition.onnx -o ~/.cache/ocrs/text-recognition.onnx
```

Here's what it does:

0. `warp.rs` can first straighten out a photo of paper that isn't flat or isn't square-on. By
   default (`--dewarp auto`), that's only tried when the picture as it is gives clues that don't
   make sense (row and column totals that disagree, clues too long for their lines), and only
   kept if it makes clearly more sense straightened; it costs a second read. Each grid line is followed as a curve, snapping at each step to the
   lattice of lines measured in the patch around it, and stopping where the lines stop being
   solid (the grid has ended, or it's a column of digits, which lines up like a line but has
   gaps). Then one smooth surface is fitted to all of them (which also extends the lines past
   the grid, over the clues), and each cell is mapped to a square. `--debug-lines` draws what it
   traced (red and blue) and the fitted lines (green). Small print may need `--scale 2` too.
1. `main.rs` finds digits, with some help for what `ocrs` is bad at: lone
   "1"s, faded clues, and tightly-stacked column clues that detection sees as one blob.
2. `clue_layout.rs` decides which of those digits are clues, and from that, where the grid
   and each of its columns and rows are. (It doesn't use `ocrs` itself, but its tests, being the
   binary's, need `cargo test --features ocr`.)
   `grid.rs` can also find the grid from its lines (evenly spaced peaks of thin-line pixels),
   but it's less reliable at telling where the grid ends and clue boxes begin, so it's only
   used when the clues gave nothing sensible. It draws the lines it found in purple on the debug
   image, with the grid's bounds doubled.
3. `reread.rs` goes back to the pixels, now knowing where each lane's clues
   must be, and finds each number from the ink, then recognizes it on its own. (Measured on the
   ink, the gap between two numbers is reliably wider than between two digits of one number, even
   where "1 11" and "1 1 1" look alike to OCR.) Where that fails for a lane, step 2's reading
   stands. `--no-reread` skips this step, for comparison. When the grid was found from its
   lines (so the clues likely sit in boxes), a lane that doesn't read cleanly is read slot by
   slot instead: the clues of every lane sit at the same, regular spacing. A slot with ink that
   doesn't read as a number (say, a crossed-out clue) becomes a "blotted" clue, shown as `?`. (`--compare-digits` adds a check of
   each digit against the others recognized as the same digit, in `templates.rs`; it hasn't
   helped on the pictures we have.)
4. If the clues solve (even partly), `cells.rs` reads the state of the grid: which cells the
   person has filled in, crossed out, or left undecided. It sorts the cells into groups that look
   alike, and the answer says what each group means: one that's all filled in the answer is the
   filled cells, and so on. (A picture with only one crossed-out cell gives nothing to compare
   it to, so it's left undecided.)

`--debug-image` is the tool for figuring out what went wrong: step 2's column clues are outlined
in blue, its row clues in red, digits it ignored in gray, and text that wasn't digits in orange.
The green lines are where it thinks the grid's top and left edges are. Step 3's numbers are in
teal, labeled with what they read as, or in magenta if they didn't read as a number. Step 4's
cells are marked with a green square (filled), a red X (crossed out), or a gray dot (undecided).
`--dump-glyphs` prints what OCR found in step 1.

To measure accuracy, keep pictures in a directory with a hand-checked `NAME.clues` beside each
`NAME.png` (or `.webp`, etc.), and run `ocr-clues --score DIR` (with `--debug-dir` to get every
picture's debug image). For a picture without one, it writes what it read, marked `UNCHECKED`;
correct it, delete that line, and it counts from then on. (The format is in
`clue_layout::Expected`.) The pictures this was developed against came from r/nonograms, so
they're kept out of the repo.

Two warnings are worth taking seriously: the row and column clues filling different numbers of
cells means a digit was misread or missed, and a line that's too long for the grid often means
two clues got read as one number.
