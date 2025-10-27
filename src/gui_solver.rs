use crate::{
    grid_solve::LineStatus,
    gui::{CanvasGui, Dirtiness, Disambiguator, Tool},
    puzzle::{Color, DynPuzzle, Solution},
};
use egui::{Color32, Pos2, Rect, Vec2, text::Fonts};

pub struct SolveGui {
    pub canvas: CanvasGui,
    pub clues: DynPuzzle,
    pub intended_solution: Solution,
    pub analyze_lines: bool,
    pub detect_errors: bool,
    pub line_analysis: Option<(Vec<LineStatus>, Vec<LineStatus>)>,
    pub render_style: RenderStyle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderStyle {
    TraditionalDots,
    TraditionalXes,
    Experimental,
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
            analyze_lines: false,
            detect_errors: false,
            line_analysis: None,
            render_style: RenderStyle::Experimental,
        }
    }

    fn detect_any_errors(&self) -> bool {
        for (x, row) in self.canvas.picture.grid.iter().enumerate() {
            for (y, color) in row.iter().enumerate() {
                if *color != self.intended_solution.grid[x][y] && *color != crate::puzzle::UNSOLVED
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

            ui.label("Render style");
            ui.radio_value(
                &mut self.render_style,
                RenderStyle::TraditionalDots,
                "traditional (dots mark background)",
            );
            ui.radio_value(
                &mut self.render_style,
                RenderStyle::TraditionalXes,
                "traditional (Xes mark background)",
            );
            ui.radio_value(
                &mut self.render_style,
                RenderStyle::Experimental,
                "experimental (pattern marks unsolved)",
            );

            ui.separator();

            ui.checkbox(&mut self.analyze_lines, "[auto]");
            if ui.button("Analyze Lines").clicked() || self.analyze_lines {
                self.line_analysis = Some(match &self.clues {
                    DynPuzzle::Nono(puzzle) => {
                        let grid =
                            crate::grid_solve::grid_from_solution(&self.canvas.picture, puzzle);
                        crate::grid_solve::analyze_lines(puzzle, &grid)
                    }
                    DynPuzzle::Triano(puzzle) => {
                        let grid =
                            crate::grid_solve::grid_from_solution(&self.canvas.picture, puzzle);
                        crate::grid_solve::analyze_lines(puzzle, &grid)
                    }
                });
            }

            ui.separator();

            ui.checkbox(&mut self.detect_errors, "[auto]");
            if ui.button("Detect errors").clicked() || self.detect_errors {
                if self.detect_any_errors() {
                    ui.colored_label(egui::Color32::RED, "Error detected");
                }
            }
            if self.is_correctly_solved() {
                ui.colored_label(egui::Color32::GREEN, "Correctly solved");
            }
        });
    }
}

#[derive(Clone, Copy)]
pub enum Orientation {
    Horizontal,
    Vertical,
}

use crate::line_solve::SolveMode;

fn draw_clues<C: crate::puzzle::Clue>(
    ui: &mut egui::Ui,
    puzzle: &crate::puzzle::Puzzle<C>,
    scale: f32,
    orientation: Orientation,
    line_analysis: Option<&[LineStatus]>,
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

    let puzz_padding = 10.0;
    let between_clues = scale * 0.5;
    let box_side = scale * 0.9;
    let box_margin = (scale - box_side) / 2.0;

    let clues_vec = match orientation {
        Orientation::Horizontal => &puzzle.rows,
        Orientation::Vertical => &puzzle.cols,
    };

    let mut max_size: f32 = 0.0;
    for line_clues in clues_vec {
        let mut this_size = 0.0;
        for clue in line_clues {
            this_size += box_side * (clue.express(puzzle).len() as f32) + between_clues;
        }
        max_size = max_size.max(this_size);
    }
    max_size += puzz_padding;

    let (response, painter) = ui.allocate_painter(
        match orientation {
            Orientation::Horizontal => Vec2::new(max_size, scale * puzzle.rows.len() as f32),
            Orientation::Vertical => Vec2::new(scale * puzzle.cols.len() as f32, max_size),
        } + Vec2::new(2.0, 2.0),
        egui::Sense::empty(),
    );

    for i in 0..clues_vec.len() {
        if let Some(analysis) = line_analysis {
            let center = match orientation {
                Orientation::Horizontal => Pos2::new(
                    response.rect.max.x - puzz_padding / 2.0,
                    response.rect.min.y + (i as f32 + 0.5) * scale,
                ),
                Orientation::Vertical => Pos2::new(
                    response.rect.min.x + (i as f32 + 0.5) * scale,
                    response.rect.max.y - puzz_padding / 2.0,
                ),
            };
            let radius = scale * 0.2;

            match &analysis[i] {
                Ok(Some(SolveMode::Skim)) => {
                    painter.circle_filled(center, radius, Color32::BLACK);
                }
                Ok(Some(SolveMode::Scrub)) => {
                    let points = vec![
                        center + Vec2::new(0.0, -radius),
                        center + Vec2::new(radius, 0.0),
                        center + Vec2::new(0.0, radius),
                        center + Vec2::new(-radius, 0.0),
                    ];
                    painter.add(egui::Shape::convex_polygon(
                        points,
                        Color32::BLACK,
                        egui::Stroke::NONE,
                    ));
                }
                Err(_) => {
                    let stroke = egui::Stroke::new(2.0, Color32::RED);
                    painter.line_segment(
                        [
                            center + Vec2::new(-radius, -radius),
                            center + Vec2::new(radius, radius),
                        ],
                        stroke,
                    );
                    painter.line_segment(
                        [
                            center + Vec2::new(radius, -radius),
                            center + Vec2::new(-radius, radius),
                        ],
                        stroke,
                    );
                }
                _ => {}
            }
        }

        let line_clues = &clues_vec[i];
        let mut current_pos = match orientation {
            Orientation::Horizontal => response.rect.max.x - puzz_padding,
            Orientation::Vertical => response.rect.max.y - puzz_padding,
        };

        for clue in line_clues.iter().rev() {
            let expressed_clues = clue.express(puzzle);

            for (color_info, len) in expressed_clues.into_iter().rev() {
                let (r, g, b) = color_info.rgb;
                let bg_color = egui::Color32::from_rgb(r, g, b);

                let corner = match orientation {
                    Orientation::Horizontal => Pos2::new(
                        current_pos,
                        response.rect.min.y + (i as f32) * scale + box_margin,
                    ),
                    Orientation::Vertical => Pos2::new(
                        response.rect.min.x + (i as f32) * scale + box_margin,
                        current_pos,
                    ),
                };

                if let Some(len) = len {
                    assert!(len > 0);

                    let clue_txt = len.to_string();
                    let clue_font = fonts_by_digit[clue_txt.len()].clone();

                    let translated_corner = corner
                        + match orientation {
                            Orientation::Horizontal => Vec2::new(-box_side, 0.0),
                            Orientation::Vertical => Vec2::new(0.0, -box_side),
                        };

                    painter.rect_filled(
                        Rect::from_min_size(translated_corner, Vec2::new(box_side, box_side)),
                        0.0,
                        bg_color,
                    );
                    painter.text(
                        translated_corner + Vec2::new(box_side / 2.0, box_side / 2.0),
                        egui::Align2::CENTER_CENTER,
                        clue_txt,
                        clue_font,
                        egui::Color32::WHITE,
                    );
                    current_pos -= box_side;
                } else {
                    let mut triangle = crate::gui::triangle_shape(
                        color_info.corner.expect("must be a corner"),
                        bg_color,
                        Vec2::new(box_side, box_side),
                    );
                    let translated_corner = corner
                        + match orientation {
                            Orientation::Horizontal => Vec2::new(-box_side, 0.0),
                            Orientation::Vertical => Vec2::new(0.0, -box_side),
                        };
                    triangle.translate(translated_corner.to_vec2());
                    current_pos -= box_side;

                    painter.add(triangle);
                }
            }
            current_pos -= between_clues;
        }
    }
}

pub fn draw_dyn_clues(
    ui: &mut egui::Ui,
    puzzle: &DynPuzzle,
    scale: f32,
    orientation: Orientation,
    line_analysis: Option<&[LineStatus]>,
) {
    match puzzle {
        DynPuzzle::Nono(puzzle) => {
            draw_clues::<crate::puzzle::Nono>(ui, puzzle, scale, orientation, line_analysis);
        }
        DynPuzzle::Triano(puzzle) => {
            draw_clues::<crate::puzzle::Triano>(ui, puzzle, scale, orientation, line_analysis);
        }
    }
}
