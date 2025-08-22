pub mod export;
pub mod grid_solve;
pub mod gui;
pub mod import;
pub mod line_solve;
pub mod puzzle;

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
        "apron.png                                skims:     77  scrubs:      0  cells left: 0",
        "bill_jeb_and_bob.png                     skims:    249  scrubs:      2  cells left: 0",
        "boring_blob.png                          skims:     32  scrubs:      0  cells left: 0",
        "boring_blob_large.png                    skims:    103  scrubs:      0  cells left: 0",
        "boring_hollow_blob.png                   skims:     34  scrubs:      0  cells left: 0",
        "carry_on_bag.png                         skims:     77  scrubs:     29  cells left: 0",
        "clock.png                                skims:    165  scrubs:     16  cells left: 0",
        "compact_fluorescent_lightbulb.png        skims:    264  scrubs:      3  cells left: 0",
        "ear.png                                  skims:    225  scrubs:     24  cells left: 0",
        "fire_submarine.png                       skims:    161  scrubs:      0  cells left: 0",
        "hair_dryer.png                           skims:    144  scrubs:     20  cells left: 0",
        "headphones.png                           skims:    415  scrubs:     11  cells left: 0",
        "keys.png                                 skims:     62  scrubs:      0  cells left: 0",
        "ladle.png                                skims:     20  scrubs:      0  cells left: 0",
        "myst_falling_man.png                     skims:     63  scrubs:     14  cells left: 0",
        "pill_bottles.png                         skims:    235  scrubs:     15  cells left: 0",
        "puzzle_piece.png                         skims:     73  scrubs:      0  cells left: 0",
        "ringed_planet.png                        skims:    159  scrubs:     22  cells left: 0",
        "shirt_and_tie.png                        skims:    308  scrubs:     30  cells left: 0",
        "shirt_and_tie_no_button.png              skims:    185  scrubs:     47  cells left: 246",
        "skid_steer.png                           skims:    203  scrubs:      1  cells left: 0",
        "stroller.png                             skims:    124  scrubs:     77  cells left: 406",
        "sunglasses.png                           skims:    185  scrubs:     23  cells left: 0",
        "tandem_stationary_bike.png               skims:    336  scrubs:     43  cells left: 0",
        "tea.png                                  skims:    100  scrubs:      0  cells left: 0",
        "tedious_dust_10x10.png                   skims:     90  scrubs:     22  cells left: 0",
        "tedious_dust_25x25.png                   skims:    524  scrubs:     88  cells left: 0",
        "tedious_dust_30x30.png                   skims:    973  scrubs:    225  cells left: 0",
        "tedious_dust_40x40.png                   skims:   1478  scrubs:    338  cells left: 0",
        "telephone_recevier.png                   skims:     34  scrubs:      0  cells left: 0",
        "tissue_box.png                           skims:     64  scrubs:     50  cells left: 148",
        "tornado.png                              skims:     96  scrubs:     15  cells left: 0",
        "usb_type_a.png                           skims:    308  scrubs:     56  cells left: 0",
        "usb_type_a_no_emblem.png                 skims:    345  scrubs:     82  cells left: 0",
    ];

    for line in expected_report {
        assert!(report.contains(line));
    }

    assert_eq!(report.lines().collect::<Vec<_>>().len(), 34);
}
