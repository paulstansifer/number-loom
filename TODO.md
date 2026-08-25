* Switch the metadata from `Option<String>` to `String` (and treat the empty string as "absent"); this will simplify editing.
* Store an ID, per webpbn format (use the hash)
* Persistent K/V store: store which puzzles are solved and remember the author's name
* Maybe rename "gui_solver.rs"; it's too similar to "grid_solve.rs".
* https://github.com/emilk/egui/issues/3218 has a workaround for bold text (for puzzle titles)
* Maybe investigate using Tauri?
* The "unsolved cell" dots are too big in triddlers.
* If the user drags the line tool off of a lane, maybe we should keep the rosette and helper numbers locked to the original lane?
* "New" dialog for triddlers needs work. Perhaps we should show a resizer and a preview?