pub mod export;
pub mod formats;
pub mod geometry;
pub mod gui;
pub mod import;
pub mod layout;
pub mod puzzle;
pub mod solve;
pub mod user_settings;

#[cfg(test)]
use crate::puzzle::PuzzleDynOps;

#[test]
// This is a consistency test, used to notice when measured difficulties change.
fn solve_examples() {
    use crate::{import, solve::grid_solve::Report};
    use itertools::Itertools;
    use std::path::PathBuf;

    let examples_dir = PathBuf::from("examples/png");
    let mut report = String::new();
    for entry in std::fs::read_dir(examples_dir)
        .unwrap()
        .into_iter()
        .sorted_by_key(|entry| entry.as_ref().unwrap().path().to_str().unwrap().to_string())
    {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_file() {
            let mut document = import::load_path(&path, None).unwrap();
            match document.puzzle().plain_solve() {
                Ok(Report {
                    solve_counts,
                    cells_left,
                    solution: _solution,
                    solved_mask: _solved_mask,
                }) => {
                    let filename = path.file_name().unwrap().to_str().unwrap();
                    report.push_str(&format!(
                        "{filename: <40} {solve_counts}  cells left: {cells_left}\n"
                    ));
                }
                Err(e) => {
                    panic!("{path:?}: internal error: {e:?}");
                }
            }
        }
    }

    println!("{}", report);

    let expected_report = vec![
        "apron.png                                skims:     80  scrubs:      0  cells left: 0",
        "bill_jeb_and_bob.png                     skims:    179  scrubs:      0  cells left: 0",
        "boring_blob.png                          skims:     35  scrubs:      0  cells left: 0",
        "boring_blob_large.png                    skims:    109  scrubs:      0  cells left: 0",
        "boring_hollow_blob.png                   skims:     37  scrubs:      0  cells left: 0",
        "carry_on_bag.png                         skims:     78  scrubs:     12  cells left: 0",
        "clock.png                                skims:    149  scrubs:     30  cells left: 0",
        "compact_fluorescent_lightbulb.png        skims:    298  scrubs:     15  cells left: 0",
        "ear.png                                  skims:    229  scrubs:     14  cells left: 0",
        "fire_submarine.png                       skims:    166  scrubs:      0  cells left: 0",
        "hair_dryer.png                           skims:    141  scrubs:     24  cells left: 0",
        "headphones.png                           skims:    269  scrubs:      2  cells left: 0",
        "keys.png                                 skims:     64  scrubs:      0  cells left: 0",
        "ladle.png                                skims:     18  scrubs:      0  cells left: 0",
        "myst_falling_man.png                     skims:     67  scrubs:      6  cells left: 0",
        "number_loom.png                          skims:    171  scrubs:     16  cells left: 0",
        "pill_bottles.png                         skims:    221  scrubs:     16  cells left: 0",
        "puzzle_piece.png                         skims:     64  scrubs:      0  cells left: 0",
        "ringed_planet.png                        skims:    162  scrubs:      5  cells left: 0",
        "shirt_and_tie.png                        skims:    205  scrubs:     19  cells left: 0",
        "shirt_and_tie_no_button.png              skims:     86  scrubs:     42  cells left: 236",
        "skid_steer.png                           skims:    120  scrubs:      0  cells left: 0",
        "stroller.png                             skims:    353  scrubs:     50  cells left: 0",
        "sunglasses.png                           skims:    198  scrubs:     16  cells left: 0",
        "tandem_stationary_bike.png               skims:    236  scrubs:     29  cells left: 0",
        "tea.png                                  skims:     85  scrubs:      0  cells left: 0",
        "tedious_dust_10x10.png                   skims:     77  scrubs:      9  cells left: 0",
        "tedious_dust_25x25.png                   skims:    513  scrubs:    127  cells left: 0",
        "tedious_dust_30x30.png                   skims:   1022  scrubs:    104  cells left: 0",
        "tedious_dust_40x40.png                   skims:   1480  scrubs:    150  cells left: 0",
        "telephone_recevier.png                   skims:     37  scrubs:      0  cells left: 0",
        "tissue_box.png                           skims:    144  scrubs:     47  cells left: 0",
        "tornado.png                              skims:     98  scrubs:     16  cells left: 0",
        "usb_type_a.png                           skims:    298  scrubs:     13  cells left: 0",
        "usb_type_a_no_emblem.png                 skims:    299  scrubs:     14  cells left: 0",
    ];

    for line in expected_report {
        assert!(report.contains(line), "expected '{}'", line);
    }

    assert_eq!(report.lines().collect::<Vec<_>>().len(), 35);
}

#[test]
// As `solve_examples`, but for the triddlers under `examples/triddler`. A separate test since
// that directory also holds a `README.md` that isn't a puzzle file, and since a triddler's
// `Puzzle` is a different type from a square one's.
fn solve_triddler_examples() {
    use crate::{import, solve::grid_solve::Report};
    use std::path::PathBuf;

    let examples_dir = PathBuf::from("examples/triddler");
    let mut solved_any = false;
    for entry in std::fs::read_dir(examples_dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("g") {
            continue;
        }

        let mut document = import::load_path(&path, None).unwrap();
        let puzzle = document
            .puzzle()
            .as_tri_nono()
            .unwrap_or_else(|| panic!("{path:?}: expected a triddler"));
        let Report { cells_left, .. } = puzzle
            .plain_solve()
            .unwrap_or_else(|e| panic!("{path:?}: internal error: {e:?}"));
        assert_eq!(cells_left, 0, "{path:?}: should solve by line logic alone");
        solved_any = true;
    }
    assert!(solved_any, "no triddler examples found to solve");
}
