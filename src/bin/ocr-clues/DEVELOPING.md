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

1. `main.rs` finds digits, with some help for what `ocrs` is bad at: lone
   "1"s, faded clues, and tightly-stacked column clues that detection sees as one blob.
2. `clue_layout.rs` decides which of those digits are clues, and from that, where the grid
   and each of its columns and rows are. (It doesn't use `ocrs` itself, but its tests, being the
   binary's, need `cargo test --features ocr`.)
3. `reread.rs` goes back to the pixels, now knowing where each lane's clues
   must be, and finds each number from the ink, then recognizes it on its own. (Measured on the
   ink, the gap between two numbers is reliably wider than between two digits of one number, even
   where "1 11" and "1 1 1" look alike to OCR.) Where that fails for a lane, step 2's reading
   stands. `--no-reread` skips this step, for comparison.

`--debug-image` is the tool for figuring out what went wrong: step 2's column clues are outlined
in blue, its row clues in red, digits it ignored in gray, and text that wasn't digits in orange.
The green lines are where it thinks the grid's top and left edges are. Step 3's numbers are in
teal, labeled with what they read as, or in magenta if they didn't read as a number.
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
