use crate::{
    grid_solve::LineStatus,
    gui::{Action, ActionMood, CanvasGui, Disambiguator, Staleable, Tool},
    puzzle::{Color, DynPuzzle, PuzzleDynOps, Solution},
};
use egui::{Color32, Pos2, Rect, Vec2, text::Fonts};

use crate::puzzle::Document;
pub struct SolveGui {
    pub canvas: CanvasGui,
    pub clues: DynPuzzle,
    pub intended_solution: Solution,
    pub analyze_lines: bool,
    pub detect_errors: bool,
    pub infer_background: bool,
    pub line_analysis: Staleable<Option<(Vec<LineStatus>, Vec<LineStatus>)>>,
    pub render_style: RenderStyle,
    last_inferred_version: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderStyle {
    TraditionalDots,
    TraditionalXes,
    Experimental,
}

impl SolveGui {
    pub fn new(
        document: Document,
        clues: DynPuzzle,
        current_color: Color,
        intended_solution: Solution,
    ) -> Self {
        let picture = document.try_solution().unwrap();
        let solved_mask = vec![vec![true; picture.grid[0].len()]; picture.grid.len()];
        SolveGui {
            canvas: CanvasGui {
                document,
                version: 0,
                current_color,
                drag_start_color: current_color,
                undo_stack: vec![],
                redo_stack: vec![],
                current_tool: Tool::OrthographicLine,
                line_tool_state: None,
                solved_mask: Staleable {
                    val: ("".to_string(), solved_mask),
                    version: 0,
                },
                disambiguator: Staleable {
                    val: Disambiguator::new(),
                    version: 0,
                },
            },
            clues,
            intended_solution,
            analyze_lines: false,
            detect_errors: false,
            infer_background: false,
            line_analysis: Staleable {
                val: None,
                version: u32::MAX,
            },
            render_style: RenderStyle::Experimental,
            last_inferred_version: u32::MAX,
        }
    }

    fn detect_any_errors(&self) -> bool {
        let picture = self.canvas.document.try_solution().unwrap();
        for (x, row) in picture.grid.iter().enumerate() {
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
        self.canvas.document.try_solution().unwrap().grid == self.intended_solution.grid
    }

    fn infer_background(&mut self) {
        let picture = self.canvas.document.solution_mut().unwrap();
        let mut grid = picture.to_partial();

        if self.clues.settle_solution(&mut grid).is_ok() {
            let mut changes = std::collections::HashMap::new();
            for ((y, x), cell) in grid.indexed_iter() {
                let current_color = picture.grid[x][y];
                if cell.is_known() && cell.known_or() != Some(current_color) {
                    changes.insert((x, y), cell.known_or().unwrap());
                }
            }

            if !changes.is_empty() {
                self.canvas
                    .perform(Action::ChangeColor { changes }, ActionMood::Merge);
            }
        }
    }

    pub fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.vertical(|ui| {
            ui.set_width(120.0);

            if let Some(title) = &self.canvas.document.title {
                ui.label(title);
            }
            if let Some(author) = &self.canvas.document.author {
                ui.label(author);
            }

            self.canvas.common_sidebar_items(ui, true);

            ui.separator();

            ui.label("Render style");
            ui.radio_value(
                &mut self.render_style,
                RenderStyle::TraditionalDots,
                "traditional (dots)",
            );
            ui.radio_value(
                &mut self.render_style,
                RenderStyle::TraditionalXes,
                "traditional (Xes)",
            );
            ui.radio_value(
                &mut self.render_style,
                RenderStyle::Experimental,
                "experimental",
            );

            ui.separator();

            ui.checkbox(&mut self.analyze_lines, "[auto]");
            if ui.button("Analyze Lines").clicked() || self.analyze_lines {
                let clues = &self.clues;
                let picture = self.canvas.document.try_solution().unwrap();
                let grid = picture.to_partial();
                self.line_analysis
                    .get_or_refresh(self.canvas.version, || Some(clues.analyze_lines(&grid)));
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

                if let Some(desc) = &self.canvas.document.description {
                    ui.label(desc);
                }
            }

            ui.separator();

            ui.checkbox(&mut self.infer_background, "[auto]");
            if ui.button("Infer background").clicked() || self.infer_background {
                if self.last_inferred_version != self.canvas.version {
                    self.infer_background();
                    self.last_inferred_version = self.canvas.version;
                }
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
    is_stale: bool,
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
            let color = if is_stale {
                Color32::from_gray(192)
            } else {
                Color32::BLACK
            };

            match &analysis[i] {
                Ok(Some(SolveMode::Skim)) => {
                    painter.circle_filled(center, radius, color);
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
                        color,
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
    is_stale: bool,
) {
    match puzzle {
        DynPuzzle::Nono(puzzle) => {
            draw_clues::<crate::puzzle::Nono>(
                ui,
                puzzle,
                scale,
                orientation,
                line_analysis,
                is_stale,
            );
        }
        DynPuzzle::Triano(puzzle) => {
            draw_clues::<crate::puzzle::Triano>(
                ui,
                puzzle,
                scale,
                orientation,
                line_analysis,
                is_stale,
            );
        }
    }
}
