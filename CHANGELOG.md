## Changelog

## Future
### Added
 - Annotation tool during solve to help count out lines. (Use "A" or shift-drag to activate it.)
 - There are now keyboard shortcuts for tools. Scrollwheel selects colors, and middle-drag pans the canvas.
 - There is now a backtracking solver, for non-line-logic nonograms
 - Chargrid now supports triddlers.
 - It's now possible to manually or (optionally) automatically indicate that clues have been satisfied.
 - All puzzles are now supported for HTML export, which now uses SVG
### Changed
 - Triddler sizes are now shown in griddlers.net's notation (e.g. "(5+3)x(6+2)")
 - A simpler way to determine which line to look at while solving is a substantial performance improvement, but it changes how scores are calculated.
### Removed
 - Removed the pencil tool from solve mode; I don't believe it's ever the right choice.

## 0.5.0 - 2026-08-25
### Added
 - Added support for triddlers (triangles on a hex grid)
 - Added lasso select-and-move
 - Added a status bar to the GUI
 - Added a replay of the solve process on completion in solve mode
 - Added a count of the whole contiguous line in the clue gutter in solve mode
    (this was inspired by the Webpbn solver)
### Fixed
 - Loading an unsolveable puzzle would cause a crash.
 - Removed various panics on malformed input.
### Internal improvements
 - Substantial solver performance improvements
    (some algorithmic improvements, but mostly avoiding unnecessary text and console operations)
 - Changed the WOVEN format (since nobody is using it yet) to be backwards-compatible in the future

## 0.4.2 - 2025-12-18
### Fixed
 - A bug in scrubbing meant that we occasionally didn't make all the deductions we could.

## 0.4.1 - 2025-11-20
### Added
 - Metadata editing
 - WOVEN format for copy-and-pasting puzzles
 - Cell counter assitance in solve mode

## 0.4.0 - 2025-10-30
### Added
 - Solve mode (including various useful helper analyses)
 - A small library of example nonograms, viewable in a spoiler-free manner.
 - Orthographic line tool
 - Support for puzzle metadata (only imported or exported for the `webpbn` format so far)

## 0.3.0 - 2025-08-22
### Added
 - Line-solving is now exhaustive!
 - Bucket fill tool, in addition to the pencil tool
### Internal improvements
 - Factored into a library with a binary wrapper
 - Added simple benchmarks

## 0.2.2 - 2025-06-18
### Added
 - Can now run on the Web
### Fixed
 - Various minor GUI improvements

## 0.2.1 - 2025-06-11
### Added
 - disambiguator cache, making it somewhat faster
 - command-line invocation for the disambiguator
### Fixed
 - `olsak` trianograms weren't round-tripping correctly

## 0.2.0 - 2025-06-06
### Added
 - a GUI
 - a disambiguator to find single-cell changes that improve solveability
 - read support for the `olsak` format.
 - support for trianograms
### Other
 - Name changed from `convert-nonogram` to `number-loom`

## 0.1.2 - 2020-11-24
### Fixed
 - `--olsak` files weren't readable by Nonny.

## 0.1.1 - 2020-11-18
### Added 
 - Added warnings for various degenerate nonograms.
### Fixed
 - `--output` wasn't working.

## 0.1.0 - 2020-11-18
### Added 
 - Convert from images to `netpbn` and `olsak` nonogram formats.