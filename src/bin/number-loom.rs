use std::path::PathBuf;

use clap::Parser;
use colored::Colorize;
use number_loom::import;
use number_loom::import::quality_check;
use number_loom::puzzle::NonogramFormat;
use number_loom::puzzle::PuzzleDynOps;
use number_loom::puzzle::{Solution, Document};
use number_loom::{export, grid_solve, gui};

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
            gui::edit_image(Document::from_solution(
                Solution::blank_bw(20, 20),
                "blank.xml".to_owned(),
            ));
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
        gui::edit_image(document);
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

        None => {
            let options = grid_solve::SolveOptions {
                trace_solve: args.trace_solve,
                display_cli_progress: true,
                ..Default::default()
            };

            match document.puzzle().solve(&options) {
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
            }
        }
    }

    Ok(())
}
