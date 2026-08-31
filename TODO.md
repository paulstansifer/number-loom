* Switch the metadata from `Option<String>` to `String` (and treat the empty string as "absent"); this will simplify editing.
* Store an ID, per webpbn format (use the hash)
* Persistent K/V store: store which puzzles are solved and remember the author's name
* https://github.com/emilk/egui/issues/3218 has a workaround for bold text (for puzzle titles)
* The "unsolved cell" dots are too big in triddlers.
* HTML export support for all puzzle types
* Webpbn import by ID
* Multicolor trianograms
* Manual + automatic clue cross-off
* Lock clues onscreen in solve mode
* Actually record useful statistics in backtracking mode
* Let the picker choose how long to keep going at a particular level (instead of having a fixed budget)
* Write information into children instead of nuking them

# Maybe?
* Human-like reasoning modes for difficulty measurement (and a max-effort slider in the GUI)
* Bottleneck difficulty measurement during solve
* Scale+quantize a picture into a puzzle
* Inverted "disambiguate" to increase difficulty
* PWA + touch input with cursor
* If the user drags the line tool off of a lane, keep the rosette and helper numbers locked to the original lane
* investigate using Tauri?
* Come up with terminology that distinguishes between the GUI for people to use while solving and the auto-solver.
