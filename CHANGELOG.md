## Changelog

## Current version
### Added
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