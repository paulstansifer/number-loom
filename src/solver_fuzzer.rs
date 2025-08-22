#![allow(dead_code)] // Otherwise, anything not tested by this becomes a warning!

mod export;
mod grid_solve;
mod gui;
mod import;
mod line_solve;
mod puzzle;

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use crate::import::{solution_to_puzzle, solution_to_triano_puzzle};
    use crate::line_solve::{Cell, exhaust_line, scrub_line, skim_line};
    use crate::puzzle::{BACKGROUND, Clue, Color, ColorInfo, Puzzle, Solution};
    use ndarray::Array1;
    use rand::{Rng, SeedableRng};

    fn generate_random_line(length: usize, num_colors: u8) -> Vec<Color> {
        let mut rng = rand::thread_rng();
        let mut line = Vec::with_capacity(length);
        let mut current_color = if rng.gen_bool(0.5) {
            BACKGROUND
        } else {
            Color(rng.gen_range(1..=num_colors))
        };
        let mut current_run_length = 0;

        for _ in 0..length {
            if current_run_length == 0 {
                let previous_color = current_color;
                // Make consecutive runs have different colors!
                while current_color == previous_color {
                    current_color = if rng.gen_bool(0.5) {
                        BACKGROUND
                    } else {
                        Color(rng.gen_range(1..=num_colors))
                    };
                }
                current_run_length = rng.gen_range(1..=(length / 2).max(1));
            }
            line.push(current_color);
            current_run_length -= 1;
        }
        line
    }

    fn generate_consistent_partial_solution(
        solution_line: &[Color],
        num_colors: u8,
    ) -> Array1<Cell> {
        let mut rng = rand::thread_rng();
        let mut partial_solution = Vec::with_capacity(solution_line.len());

        for &actual_color in solution_line {
            let mut cell = Cell::new_impossible();
            cell.actually_could_be(actual_color); // Must allow the actual color

            // For other colors, 75% chance of also allowing it
            for i in 0..num_colors {
                let other_color = Color(i);
                if other_color != actual_color && rng.gen_bool(0.75) {
                    cell.actually_could_be(other_color);
                }
            }
            partial_solution.push(cell);
        }
        Array1::from(partial_solution)
    }

    fn dummy_color(color: Color) -> (Color, ColorInfo) {
        (
            color,
            crate::puzzle::ColorInfo {
                ch: ' ',
                name: String::new(),
                rgb: (0, 0, 0),
                color,
                corner: if color.0 <= 1 {
                    None
                } else {
                    Some(crate::puzzle::Corner {
                        left: color.0 % 2 == 0,
                        upper: true,
                    })
                },
            },
        )
    }

    fn validate_solver<C: Clue, F>(case: usize, line: Vec<Color>, partial: Array1<Cell>, f: F)
    where
        F: FnOnce(&Solution) -> Puzzle<C>,
    {
        let mut available_colors = HashSet::<Color>::new();
        // Create a dummy Solution struct to use solution_to_puzzle
        let mut grid = vec![vec![BACKGROUND]; line.len()];
        for (j, color) in line.iter().enumerate() {
            grid[j][0] = *color;
            available_colors.insert(*color);
        }

        let dummy_solution = Solution {
            clue_style: crate::puzzle::ClueStyle::Nono,
            palette: available_colors.into_iter().map(dummy_color).collect(),
            grid,
        };

        let puzzle = f(&dummy_solution);
        let clues = &puzzle.rows[0]; // Get clues for the generated line

        let mut sc_partial_solution = partial.clone();
        let mut sk_partial_solution = partial.clone();

        match skim_line(clues, &mut sk_partial_solution.view_mut()) {
            Ok(_) => {
                for j in 0..line.len() {
                    if !sk_partial_solution[j].can_be(line[j]) {
                        panic!(
                            "Fuzz case {case}: skim_line inconsistent at {j}.  Clues: {:?}. Orig: {line:?}, Partial: {partial:?}, Partial solution after skim: {:?}",
                            clues, sk_partial_solution
                        );
                    }
                }
            }
            Err(e) => {
                panic!(
                    "Fuzz case {case}: skim_line error: {}. Orig: {line:?}, Partial: {partial:?}",
                    e
                );
            }
        }

        match scrub_line(clues, &mut sk_partial_solution.view_mut()) {
            Ok(_) => {
                for j in 0..line.len() {
                    if !sk_partial_solution[j].can_be(line[j]) {
                        panic!(
                            "Fuzz case {case}: scrub_line inconsistent at {j}.  Clues: {:?}. Orig: {line:?}, Partial: {partial:?}, Partial solution after scrub: {:?}",
                            clues, sk_partial_solution
                        );
                    }
                }
            }
            Err(e) => {
                panic!(
                    "Fuzz case {case}: scrub_line error: {}. Orig: {line:?}, Partial: {partial:?}",
                    e
                );
            }
        }

        match exhaust_line(clues, &mut sc_partial_solution.view_mut()) {
            Ok(_) => {
                for j in 0..line.len() {
                    if !sc_partial_solution[j].can_be(line[j]) {
                        panic!(
                            "Fuzz case {case}: exhaust_line inconsistent at {j}.  Clues: {:?}. Orig: {line:?}, Partial: {partial:?}, Partial solution after skim: {:?}",
                            clues, sc_partial_solution
                        );
                    }
                }
            }
            Err(e) => {
                panic!(
                    "Fuzz case {case}: exhaust_line error: {}. Orig: {line:?}, Partial: {partial:?}",
                    e
                );
            }
        }
    }

    #[test]
    fn fuzzer() {
        let mut rng = rand::rngs::StdRng::seed_from_u64(0);
        let num_fuzz_cases = 200;
        let max_line_length = 25;

        for i in 0..num_fuzz_cases {
            // Look at fewer colors at first:
            for max_colors in 2..=5 {
                // Look at shorter lines at first:
                let line_length = rng.gen_range(1..=max_line_length.min((i + 1) * 2));
                let solution_line = generate_random_line(line_length, max_colors);

                let original_partial_solution =
                    generate_consistent_partial_solution(&solution_line, max_colors);

                validate_solver(
                    i,
                    solution_line.clone(),
                    original_partial_solution.clone(),
                    solution_to_puzzle,
                );

                validate_solver(
                    i,
                    solution_line,
                    original_partial_solution,
                    solution_to_triano_puzzle,
                );
            }
        }
    }
}
