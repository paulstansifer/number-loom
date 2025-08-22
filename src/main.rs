extern crate clap;
extern crate image;

mod export;
mod grid_solve;
mod gui;
mod import;
mod line_solve;
mod puzzle;
use std::path::PathBuf;

use clap::Parser;
use import::quality_check;
use puzzle::NonogramFormat;

use crate::puzzle::Solution;

#[derive(clap::Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Input path; use "-" for stdin
    input_path: Option<PathBuf>,

    /// Output path for format conversion; use "-" for stdout.
    /// If omitted, solves the nonogram and reports on the difficulty.
    output_path: Option<PathBuf>,

    /// Format to expect the input to be in
    #[arg(short, long, value_enum)]
    input_format: Option<NonogramFormat>,

    /// Format to emit as output
    #[arg(short, long, value_enum)]
    output_format: Option<NonogramFormat>,

    /// Explain the solve process line-by-line.
    #[arg(short, long, action = clap::ArgAction::SetTrue)]
    trace_solve: bool,

    /// Opens the GUI editor
    #[arg(long, default_value_t)]
    gui: bool,

    #[arg(long, default_value_t)]
    disambiguate: bool,
}

fn main() -> std::io::Result<()> {
    let args = Args::parse();

    let input_path = match args.input_path {
        Some(ip) => ip,
        None => {
            gui::edit_image(Solution::blank_bw(20, 20));
            return Ok(());
        }
    };

    let mut document = import::load_path(&input_path, args.input_format);
    if let Some(ref solution) = document.try_solution() {
        quality_check(solution);
    }

    if args.gui {
        // TODO: this sorta duplicates some code in gui
        // TODO: check the solution is complete!
        let solution = document.take_solution().expect("impossible puzzle");
        gui::edit_image(solution);
        return Ok(());
    } else if args.disambiguate {
        let solution = document.take_solution().expect("impossible puzzle");

        let disambig = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(grid_solve::disambig_candidates(
                &solution,
                std::sync::mpsc::channel().0,
                std::sync::mpsc::channel().1,
            ));

        let mut best_result = f32::MAX;
        for row in &disambig {
            for cell in row {
                best_result = best_result.min(cell.1);
            }
        }

        let display_threshold = 1.0 - (1.0 - best_result) * 0.75;

        let display_threshold = if best_result == 0.0 {
            println!("Able to completely disambiguate with a one-cell change!");
            0.0
        } else {
            println!(
                "Best improvement brings ambiguities to {:0}%; showing everything {:0}% or better",
                best_result * 100.0,
                display_threshold * 100.0
            );

            display_threshold
        };

        use colored::Colorize;

        for y in 0..solution.y_size() {
            for x in 0..solution.x_size() {
                let ci = &solution.palette[&solution.grid[x][y]];
                if disambig[x][y].1 <= display_threshold {
                    let new_ch = &solution.palette[&disambig[x][y].0].ch;
                    let new_ch = if *new_ch == ' ' { '☒' } else { *new_ch };

                    print!("{}", new_ch.to_string().red())
                } else {
                    print!("{}", ci.ch)
                }
            }

            println!("");
        }

        return Ok(());
    }

    match args.output_path {
        Some(path) => {
            export::save(&mut document, &path, args.output_format).unwrap();
        }

        None => match document.puzzle().solve_with_args(args.trace_solve) {
            Ok(grid_solve::Report {
                solve_counts,
                cells_left,
                solution: _solution,
                solved_mask: _solved_mask,
            }) => {
                if cells_left == 0 {
                    eprintln!("Solved after {solve_counts}.");
                } else {
                    eprintln!(
                        "Unable to solve. Performed {solve_counts}; {cells_left} cells left."
                    );
                }
            }
            Err(e) => {
                eprintln!("Error: {:?}", e);
                std::process::exit(1);
            }
        },
    }

    Ok(())
}

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
                    panic!("{path:?}: internal error: {:?}", e);
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
