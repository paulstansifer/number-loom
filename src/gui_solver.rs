use egui::{Color32, Pos2, Rect, Vec2, text::Fonts};

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
            for (_, len) in clue.express(puzzle) {
                match len {
                    Some(_) => this_width += box_side,
                    None => this_width += box_side,
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

                let corner_u_r = Pos2::new(
                    current_x,
                    response.rect.min.y + (y as f32) * scale + box_margin,
                );

                if let Some(len) = len {
                    assert!(len > 0);

                    let clue_txt = len.to_string();
                    let clue_font = fonts_by_digit[clue_txt.len()].clone();

                    let corner_u_l = corner_u_r - Vec2::new(box_side, 0.0);

                    painter.rect_filled(
                        Rect::from_min_size(
                            corner_u_l + Vec2::new(box_margin, box_margin),
                            Vec2::new(box_side, box_side),
                        ),
                        0.0,
                        bg_color,
                    );
                    painter.text(
                        corner_u_l + Vec2::new(0.5, 0.5) * scale,
                        egui::Align2::CENTER_CENTER,
                        clue_txt,
                        clue_font,
                        egui::Color32::WHITE,
                    );
                    current_x -= box_side;
                } else {
                    let tri_width = box_side;
                    let mut triangle = crate::gui::triangle_shape(
                        color_info.corner.expect("must be a corner"),
                        bg_color,
                        Vec2::new(tri_width, box_side),
                    );
                    let corner_u_l = corner_u_r + Vec2::new(-box_side, box_margin);
                    triangle.translate(corner_u_l.to_vec2());
                    current_x -= tri_width;

                    painter.add(triangle);
                }
            }
            current_x -= between_clues;
        }
    }
}
