use egui::{Color32, Pos2, Rect, Vec2};

use crate::puzzle::{Color, DynPuzzle, Solution};

pub struct SolveGui {
    pub partial_solution: Solution,
    pub clues: DynPuzzle,
    pub current_color: Color,
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
    let font_id = egui::FontId::monospace(scale * 0.7);

    let widths = ui.fonts(|f| {
        vec![
            f.layout_no_wrap("".to_string(), font_id.clone(), Color32::BLACK)
                .rect
                .width(),
            f.layout_no_wrap("0".to_string(), font_id.clone(), Color32::BLACK)
                .rect
                .width(),
            f.layout_no_wrap("00".to_string(), font_id.clone(), Color32::BLACK)
                .rect
                .width(),
            f.layout_no_wrap("000".to_string(), font_id.clone(), Color32::BLACK)
                .rect
                .width(),
        ]
    });

    let puzz_padding = 5.0;
    let text_margin = scale * 0.18;
    let between_clues = scale * 0.5;
    let box_height = scale * 0.9;
    let box_vertical_margin = (scale - box_height) / 2.0;

    let mut max_width: f32 = 0.0;
    for row in &puzzle.rows {
        let mut this_width = 0.0;
        for clue in row {
            for (_, len) in clue.express(puzzle) {
                assert!(len.unwrap_or(1) <= 999);
                match len {
                    Some(len) => this_width += widths[len.to_string().len()] + text_margin * 2.0,
                    None => this_width += scale,
                }
            }
            this_width += between_clues;
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

                let corner_u_r = Pos2::new(current_x, response.rect.min.y + (y as f32) * scale);

                if let Some(len) = len {
                    assert!(len > 0);

                    let box_width = widths[len.to_string().len()] + text_margin * 2.0;
                    let corner_u_l = corner_u_r - Vec2::new(box_width, 0.0);

                    painter.rect_filled(
                        Rect::from_min_size(
                            corner_u_l + Vec2::new(0.0, box_vertical_margin),
                            Vec2::new(box_width, box_height),
                        ),
                        0.0,
                        bg_color,
                    );
                    painter.text(
                        // TODO: the 0.15 is tied to the constants up above:
                        corner_u_l + Vec2::new(text_margin, scale * 0.15),
                        egui::Align2::LEFT_TOP,
                        len.to_string(),
                        font_id.clone(),
                        egui::Color32::WHITE,
                    );
                    current_x -= box_width;
                } else {
                    let tri_width = box_height;
                    let mut triangle = crate::gui::triangle_shape(
                        color_info.corner.expect("must be a corner"),
                        bg_color,
                        Vec2::new(tri_width, box_height),
                    );
                    let corner_u_l = corner_u_r + Vec2::new(-box_height, box_vertical_margin);
                    triangle.translate(corner_u_l.to_vec2());
                    current_x -= tri_width;

                    painter.add(triangle);
                }
            }
            current_x -= between_clues
        }
    }
}
