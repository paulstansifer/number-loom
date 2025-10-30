* Switch the metadata from `Option<String>` to `String` (and treat the empty string as "absent"); this will simplify editing.
* Store an ID, per webpbn format (use the hash)
* Persistent K/V store: store which puzzles are solved and remember the author's name
* Maybe rename "gui_solver.rs"; it's too similar to "grid_solve.rs".
* https://github.com/emilk/egui/issues/3218 has a workaround for bold text (for puzzle titles)