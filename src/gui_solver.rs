use crate::{
    grid_solve::LineStatus,
    gui::{Action, ActionMood, CanvasGui, Disambiguator, Staleable, Tool},
    puzzle::{BACKGROUND, Color, DynPuzzle, PuzzleDynOps, UNSOLVED},
    user_settings::{UserSettings, consts},
};
use egui::{Color32, Pos2, Rect, RichText, Vec2, text::Fonts};

use crate::puzzle::{Document, DynSolution};
pub struct SolveGui {
    pub canvas: CanvasGui,
    pub clues: DynPuzzle,
    pub intended_solution: DynSolution,
    pub analyze_lines: bool,
    pub detect_errors: bool,
    pub infer_background: bool,
    pub line_analysis: Staleable<Option<(Vec<LineStatus>, Vec<LineStatus>)>>,
    pub render_style: RenderStyle,
    last_inferred_version: u32,
    pub hovered_cell: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderStyle {
    TraditionalDots,
    TraditionalXes,
    Experimental,
}

impl SolveGui {
    pub fn new(
        mut document: Document,
        status: crate::gui::SharedStatus,
        progress: crate::gui::SharedProgress,
    ) -> Self {
        let mut working_doc = document.clone();
        for cell in working_doc.solution_mut().cells_mut() {
            *cell = UNSOLVED;
        }
        working_doc.solution_mut().palette_mut().insert(
            UNSOLVED,
            crate::puzzle::ColorInfo {
                ch: '?',
                name: "unknown".to_owned(),
                rgb: (128, 128, 128),
                color: UNSOLVED,
                corner: None,
            },
        );
        let mut current_color = BACKGROUND;
        if working_doc
            .solution_mut()
            .palette_mut()
            .contains_key(&Color(1))
        {
            current_color = Color(1)
        }

        let clues = document.puzzle().clone();
        let solved_mask = vec![true; document.solution_mut().cells().len()];

        fn get_bool_setting(key: &str) -> bool {
            UserSettings::get(key)
                .and_then(|s| s.parse::<bool>().ok())
                .unwrap_or(false)
        }

        SolveGui {
            canvas: CanvasGui {
                document: working_doc,
                version: 0,
                current_color,
                drag_start_color: current_color,
                undo_stack: vec![],
                redo_stack: vec![],
                current_tool: Tool::LineAlongLane,
                line_tool_state: None,
                solved_mask: Staleable {
                    val: ("".to_string(), solved_mask),
                    version: 0,
                },
                disambiguator: Staleable {
                    val: Disambiguator::new(),
                    version: 0,
                },
                id: Staleable {
                    val: "".to_string(),
                    version: 0,
                },
                status,
                progress,
            },
            clues,
            intended_solution: document.take_solution().unwrap(),
            analyze_lines: get_bool_setting(consts::SOLVER_ANALYZE_LINES),
            detect_errors: get_bool_setting(consts::SOLVER_DETECT_ERRORS),
            infer_background: get_bool_setting(consts::SOLVER_INFER_BACKGROUND),
            line_analysis: Staleable {
                val: None,
                version: u32::MAX,
            },
            render_style: RenderStyle::Experimental,
            last_inferred_version: u32::MAX,
            hovered_cell: None,
        }
    }

    fn detect_any_errors(&self) -> bool {
        let picture = self.canvas.document.try_solution().unwrap();
        for (cell, intended) in picture.cells().iter().zip(self.intended_solution.cells()) {
            if *cell != *intended && *cell != crate::puzzle::UNSOLVED {
                return true;
            }
        }
        false
    }

    fn is_correctly_solved(&self) -> bool {
        self.canvas.document.try_solution().unwrap().cells() == self.intended_solution.cells()
    }

    fn infer_background(&mut self) {
        let mut grid = self.canvas.document.solution_mut().to_partial();

        if self.clues.settle_solution(&mut grid).is_ok() {
            let mut changes = std::collections::HashMap::new();
            let picture = self.canvas.document.try_solution().unwrap();
            for (index, cell) in grid.iter().enumerate() {
                let current_color = picture.cells()[index];
                if cell.is_known() && cell.known_or() != Some(current_color) {
                    changes.insert(index as u32, cell.known_or().unwrap());
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
            ui.set_width(150.0);

            if !self.canvas.document.title.is_empty() {
                ui.label(RichText::new(&self.canvas.document.title).strong());
            }
            if !self.canvas.document.author.is_empty() {
                ui.label(format!("by {}", &self.canvas.document.author));
            }

            self.canvas.common_sidebar_items(ui, true);

            ui.separator();
            let scale = 20.0;
            let plus_size = scale * 3.0;

            if let Some(cell) = self.hovered_cell {
                let picture = self.canvas.document.try_solution().unwrap();
                let color = picture.cells()[cell as usize];
                let rgb = picture.palette()[&color].rgb;

                // One run per clue family: two arms each for a square grid, three for a triddler.
                // `arm_directions` places them, so this becomes a hexagonal rosette by itself.
                let runs = picture.runs_at_cell(cell);
                let dirs = picture.arm_directions();

                let (resp, painter) =
                    ui.allocate_painter(Vec2::new(plus_size, plus_size), egui::Sense::empty());

                let rect = resp.rect;
                let size = Vec2::new(20.0, 20.0);
                let center = rect.min + Vec2::new(scale, scale);

                // `arm_directions` lists each family's two directions adjacently, matching the
                // `(backward, forward)` pairs `runs_at_cell` returns.
                for (family, (back, forward)) in runs.iter().enumerate() {
                    for (i, count) in [back, forward].into_iter().enumerate() {
                        let Some(dir) = dirs.get(family * 2 + i) else {
                            continue;
                        };
                        if *count == 0 {
                            continue;
                        }
                        let at = center + Vec2::new(dir.x * scale, dir.y * scale);
                        draw_string_in_box(
                            ui,
                            &painter,
                            Rect::from_min_size(at, size),
                            &count.to_string(),
                            scale,
                            rgb,
                        );
                    }
                }

                let mid_rect = Rect::from_min_size(center, size);
                if color == UNSOLVED {
                    draw_string_in_box(ui, &painter, mid_rect, "?", scale, rgb);
                } else {
                    draw_string_in_box(ui, &painter, mid_rect, " ", scale, rgb);
                }
            } else {
                ui.add_space(plus_size);
            }

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

            if ui.checkbox(&mut self.analyze_lines, "[auto]").changed() {
                let _ = UserSettings::set(
                    consts::SOLVER_ANALYZE_LINES,
                    &self.analyze_lines.to_string(),
                );
            }
            if ui.button("Analyze Lines").clicked() || self.analyze_lines {
                let clues = &self.clues;
                let picture = self.canvas.document.try_solution().unwrap();
                let grid = picture.to_partial();
                self.line_analysis
                    .get_or_refresh(self.canvas.version, || Some(clues.analyze_lines(&grid)));
            }

            ui.separator();

            if ui.checkbox(&mut self.detect_errors, "[auto]").changed() {
                let _ = UserSettings::set(
                    consts::SOLVER_DETECT_ERRORS,
                    &self.detect_errors.to_string(),
                );
            }
            if ui.button("Detect errors").clicked() || self.detect_errors {
                if self.detect_any_errors() {
                    ui.colored_label(egui::Color32::DARK_RED, "Error detected");
                }
            }
            if self.is_correctly_solved() {
                ui.colored_label(egui::Color32::DARK_GREEN, "Correctly solved");

                if !self.canvas.document.description.is_empty() {
                    ui.label(&self.canvas.document.description);
                }
            }

            ui.separator();

            if ui.checkbox(&mut self.infer_background, "[auto]").changed() {
                let _ = UserSettings::set(
                    consts::SOLVER_INFER_BACKGROUND,
                    &self.infer_background.to_string(),
                );
            }
            if ui.button("Infer background").clicked() || self.infer_background {
                if self.last_inferred_version != self.canvas.version {
                    self.infer_background();
                    self.last_inferred_version = self.canvas.version;
                }
            }
        });
    }

    pub fn body(&mut self, ui: &mut egui::Ui, scale: f32) {
        ui.vertical(|ui| {
            egui::Grid::new("solve_grid").show(ui, |ui| {
                ui.label(""); // Top-left is empty
                let is_stale = !self.line_analysis.fresh(self.canvas.version);
                let line_analysis = self.line_analysis.val.as_ref();
                draw_dyn_clues(
                    ui,
                    &self.clues,
                    scale,
                    Orientation::Vertical,
                    line_analysis.map(|la| &la.1[..]),
                    is_stale,
                );
                ui.end_row();

                draw_dyn_clues(
                    ui,
                    &self.clues,
                    scale,
                    Orientation::Horizontal,
                    line_analysis.map(|la| &la.0[..]),
                    is_stale,
                );
                self.hovered_cell = self.canvas.canvas(ui, scale, self.render_style);
                ui.end_row();
            });
        });
    }
}

#[derive(Clone, Copy)]
pub enum Orientation {
    Horizontal,
    Vertical,
}

use crate::line_solve::SolveMode;

fn draw_string_in_box(
    ui: &egui::Ui,
    painter: &egui::Painter,
    rect: Rect,
    clue_txt: &str,
    scale: f32,
    (r, g, b): (u8, u8, u8),
) {
    painter.rect_filled(rect, 0.0, Color32::from_rgb(r, g, b));
    let base_font = egui::FontId::monospace(scale * 0.7);
    let text_width = |fonts: &Fonts, t: &str| {
        fonts
            .layout_no_wrap(t.to_string(), base_font.clone(), Color32::BLACK)
            .rect
            .width()
    };
    let text_color = if r as u16 + g as u16 + b as u16 > 384 {
        Color32::BLACK
    } else {
        Color32::WHITE
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

    let clue_font = fonts_by_digit[clue_txt.len().min(fonts_by_digit.len() - 1)].clone();

    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        clue_txt,
        clue_font,
        text_color,
    );
}

fn draw_clues<C: crate::puzzle::Clue>(
    ui: &mut egui::Ui,
    puzzle: &crate::puzzle::Puzzle<C, crate::geometry::Square>,
    scale: f32,
    orientation: Orientation,
    line_analysis: Option<&[LineStatus]>,
    is_stale: bool,
) {
    let puzz_padding = 10.0;
    let between_clues = scale * 0.5;
    let box_side = scale * 0.9;
    let box_margin = (scale - box_side) / 2.0;

    let clues_vec = match orientation {
        Orientation::Horizontal => puzzle.row_clues(),
        Orientation::Vertical => puzzle.col_clues(),
    };

    let mut max_size: f32 = 0.0;
    for line_clues in clues_vec {
        let mut this_size = 0.0;
        for clue in line_clues {
            this_size += box_side * (clue.express(&puzzle.palette).len() as f32) + between_clues;
        }
        max_size = max_size.max(this_size);
    }
    max_size += puzz_padding;

    let (response, painter) = ui.allocate_painter(
        match orientation {
            Orientation::Horizontal => Vec2::new(max_size, scale * puzzle.row_clues().len() as f32),
            Orientation::Vertical => Vec2::new(scale * puzzle.col_clues().len() as f32, max_size),
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
            let expressed_clues = clue.express(&puzzle.palette);

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

                    let translated_corner = corner
                        + match orientation {
                            Orientation::Horizontal => Vec2::new(-box_side, 0.0),
                            Orientation::Vertical => Vec2::new(0.0, -box_side),
                        };

                    let rect =
                        Rect::from_min_size(translated_corner, Vec2::new(box_side, box_side));
                    draw_string_in_box(ui, &painter, rect, &len.to_string(), scale, color_info.rgb);
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
    // Clue gutters are still laid out as two axis-aligned rectangles, so only square puzzles
    // can be drawn. `Geometry::gutters` has the per-lane anchors a six-way version needs.
    match puzzle {
        DynPuzzle::SquareNono(puzzle) => {
            draw_clues(ui, puzzle, scale, orientation, line_analysis, is_stale);
        }
        DynPuzzle::SquareTriano(puzzle) => {
            draw_clues(ui, puzzle, scale, orientation, line_analysis, is_stale);
        }
        DynPuzzle::TriNono(_) => {}
    }
}
