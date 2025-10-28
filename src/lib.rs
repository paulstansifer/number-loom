pub mod export;
pub mod grid_solve;
pub mod gui;
pub mod gui_solver;
pub mod import;
pub mod line_solve;
pub mod puzzle;

#[cfg(test)]
use crate::puzzle::PuzzleDynOps;

#[test]
// This is a consistency test, used to notice when measured difficulties change.
fn solve_examples() {
    use crate::{grid_solve::Report, import};
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
            let mut document = import::load_path(&path, None);
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
        "apron.png                                skims:     77  settles:      0  scrubs:      0  cells left: 0",
        "bill_jeb_and_bob.png                     skims:    243  settles:     20  scrubs:      1  cells left: 0",
        "boring_blob.png                          skims:     32  settles:      0  scrubs:      0  cells left: 0",
        "boring_blob_large.png                    skims:    103  settles:      0  scrubs:      0  cells left: 0",
        "boring_hollow_blob.png                   skims:     34  settles:      0  scrubs:      0  cells left: 0",
        "carry_on_bag.png                         skims:     76  settles:     36  scrubs:     12  cells left: 0",
        "clock.png                                skims:    155  settles:     33  scrubs:      4  cells left: 0",
        "compact_fluorescent_lightbulb.png        skims:    280  settles:     40  scrubs:      9  cells left: 0",
        "ear.png                                  skims:    225  settles:     50  scrubs:     30  cells left: 0",
        "fire_submarine.png                       skims:    161  settles:      0  scrubs:      0  cells left: 0",
        "hair_dryer.png                           skims:    157  settles:     58  scrubs:     17  cells left: 0",
        "headphones.png                           skims:    388  settles:    110  scrubs:     11  cells left: 0",
        "keys.png                                 skims:     62  settles:      0  scrubs:      0  cells left: 0",
        "ladle.png                                skims:     20  settles:      0  scrubs:      0  cells left: 0",
        "myst_falling_man.png                     skims:     66  settles:      5  scrubs:      0  cells left: 0",
        "pill_bottles.png                         skims:    245  settles:    108  scrubs:     18  cells left: 0",
        "puzzle_piece.png                         skims:     73  settles:      0  scrubs:      0  cells left: 0",
        "ringed_planet.png                        skims:    115  settles:     10  scrubs:      1  cells left: 0",
        "shirt_and_tie.png                        skims:    308  settles:    164  scrubs:     34  cells left: 0",
        "shirt_and_tie_no_button.png              skims:    192  settles:    164  scrubs:     49  cells left: 236",
        "skid_steer.png                           skims:    203  settles:     10  scrubs:      1  cells left: 0",
        "stroller.png                             skims:    383  settles:    123  scrubs:     51  cells left: 0",
        "sunglasses.png                           skims:    185  settles:     40  scrubs:     27  cells left: 0",
        "tandem_stationary_bike.png               skims:    341  settles:    144  scrubs:     48  cells left: 0",
        "tea.png                                  skims:    100  settles:      0  scrubs:      0  cells left: 0",
        "tedious_dust_10x10.png                   skims:     88  settles:     29  scrubs:      3  cells left: 0",
        "tedious_dust_25x25.png                   skims:    535  settles:    192  scrubs:     89  cells left: 0",
        "tedious_dust_30x30.png                   skims:    965  settles:    377  scrubs:    212  cells left: 0",
        "tedious_dust_40x40.png                   skims:   1618  settles:    881  scrubs:    349  cells left: 0",
        "telephone_recevier.png                   skims:     34  settles:      0  scrubs:      0  cells left: 0",
        "tissue_box.png                           skims:    163  settles:     54  scrubs:     41  cells left: 0",
        "tornado.png                              skims:     96  settles:     22  scrubs:     20  cells left: 0",
        "usb_type_a.png                           skims:    296  settles:     37  scrubs:     42  cells left: 0",
        "usb_type_a_no_emblem.png                 skims:    319  settles:     70  scrubs:     34  cells left: 0",
    ];

    for line in expected_report {
        assert!(report.contains(line));
    }

    assert_eq!(report.lines().collect::<Vec<_>>().len(), 34);
}
