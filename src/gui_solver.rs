use crate::{
    gui::{CanvasGui, Dirtiness, Disambiguator, Tool},
    puzzle::{Color, DynPuzzle, Solution},
};
use egui::{Color32, Pos2, Rect, Vec2, text::Fonts};

pub struct SolveGui {
    pub canvas: CanvasGui,
    pub clues: DynPuzzle,
    pub intended_solution: Solution,
    pub detect_errors: bool,
}

impl SolveGui {
    pub fn new(
        picture: Solution,
        clues: DynPuzzle,
        current_color: Color,
        intended_solution: Solution,
    ) -> Self {
        let solved_mask = vec![vec![true; picture.grid[0].len()]; picture.grid.len()];
        SolveGui {
            canvas: CanvasGui {
                picture,
                dirtiness: Dirtiness::Clean,
                current_color,
                drag_start_color: current_color,
                undo_stack: vec![],
                redo_stack: vec![],
                current_tool: Tool::Pencil,
                line_tool_state: None,
                solved_mask,
                disambiguator: Disambiguator::new(),
            },
            clues,
            intended_solution,
            detect_errors: false,
        }
    }

    fn detect_any_errors(&self) -> bool {
        for (x, row) in self.canvas.picture.grid.iter().enumerate() {
            for (y, color) in row.iter().enumerate() {
                if *color != self.intended_solution.grid[x][y]
                    && *color != crate::puzzle::UNSOLVED
                {
                    return true;
                }
            }
        }
        false
    }

    fn is_correctly_solved(&self) -> bool {
        self.canvas.picture.grid == self.intended_solution.grid
    }

    pub fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.vertical(|ui| {
            ui.set_width(120.0);
            self.canvas.common_sidebar_items(ui, true);

            ui.separator();

            ui.checkbox(&mut self.detect_errors, "Detect errors");
            if self.detect_errors && self.detect_any_errors() {
                ui.colored_label(egui::Color32::RED, "Error detected");
            } else if self.is_correctly_solved() {
                ui.colored_label(egui::Color32::GREEN, "Correctly solved");
            }
        });
    }
}

pub fn draw_dyn_row_clues(ui: &mut egui::Ui, puzzle: &DynPuzzle, scale: f32) {
    match puzzle {
        DynPuzzle::Nono(puzzle) => {
            draw_row_clues::<crate::puzzle::Nono>(ui, puzzle, scale);
        }
        DynPuzzle::Triano(puzzle) => {
            draw_row_clues::<crate::puzzle::Triano>(ui, puzzle, scale);
        }
    }
}

fn draw_row_clues<C: crate::puzzle::Clue>(
    ui: &mut egui::Ui,
    puzzle: &crate::puzzle::Puzzle<C>,
    scale: f32,
) {
    let base_font = egui::FontId::monospace(scale * 0.7);

    let text_width = |fonts: &Fonts, t: &str| {
        fonts
            .layout_no_wrap(t.to_string(), base_font.clone(), Color32::BLACK)
            .rect
            .width()
    };

    let (width_2, width_3) = ui.fonts(|f| {
        (
            f32::max(text_width(f, "00") / (scale * 0.7), 1.0),
            f32::max(text_width(f, "000") / (scale * 0.7), 1.0),
        )
    });
    let fonts_by_digit = vec![
        base_font.clone(),
        base_font,
        egui::FontId::monospace(scale * 0.7 / width_2),
        egui::FontId::monospace(scale * 0.7 / width_3),
    ];

    let puzz_padding = 5.0;
    let between_clues = scale * 0.5;
    let box_side = scale * 0.9;
    let box_margin = (scale - box_side) / 2.0;

    let mut max_width: f32 = 0.0;
    for row in &puzzle.rows {
        let mut this_width = 0.0;
        for clue in row {
            this_width += box_side * (clue.express(puzzle).len() as f32) + between_clues;
        }
        max_width = max_width.max(this_width);
    }
    max_width += puzz_padding;

    let (response, painter) = ui.allocate_painter(
        Vec2::new(max_width, scale * puzzle.rows.len() as f32) + Vec2::new(2.0, 2.0),
        egui::Sense::empty(),
    );

    for y in 0..puzzle.rows.len() {
        let row_clues = &puzzle.rows[y];
        let mut current_x = response.rect.max.x - puzz_padding;

        for clue in row_clues.iter().rev() {
            let expressed_clues = clue.express(puzzle);

            for (color_info, len) in expressed_clues.into_iter().rev() {
                let (r, g, b) = color_info.rgb;
                let bg_color = egui::Color32::from_rgb(r, g, b);

                let corner_u_r = Pos2::new(
                    current_x,
                    response.rect.min.y + (y as f32) * scale + box_margin,
                );

                if let Some(len) = len {
                    assert!(len > 0);

                    let clue_txt = len.to_string();
                    let clue_font = fonts_by_digit[clue_txt.len()].clone();

                    let corner_u_l = corner_u_r + Vec2::new(-box_side, box_margin);

                    painter.rect_filled(
                        Rect::from_min_size(corner_u_l, Vec2::new(box_side, box_side)),
                        0.0,
                        bg_color,
                    );
                    painter.text(
                        corner_u_l + Vec2::new(box_side / 2.0, box_side / 2.0),
                        egui::Align2::CENTER_CENTER,
                        clue_txt,
                        clue_font,
                        egui::Color32::WHITE,
                    );
                    current_x -= box_side;
                } else {
                    let mut triangle = crate::gui::triangle_shape(
                        color_info.corner.expect("must be a corner"),
                        bg_color,
                        Vec2::new(box_side, box_side),
                    );
                    let corner_u_l = corner_u_r + Vec2::new(-box_side, box_margin);
                    triangle.translate(corner_u_l.to_vec2());
                    current_x -= box_side;

                    painter.add(triangle);
                }
            }
            current_x -= between_clues;
        }
    }
}
