use std::cmp::{max, min};
use std::collections::HashMap;

use crate::{
    gui::{cell_shape, Action, ActionMood, CanvasGui, Tool},
    puzzle::{Color, DynPuzzle, Solution, BACKGROUND, UNSOLVED},
};
use egui::{text::Fonts, Color32, Pos2, Rect, RichText, Vec2};
use egui_material_icons::icons;

pub struct SolveGui {
    pub canvas: CanvasGui,
    pub clues: DynPuzzle,
}

impl SolveGui {
    pub fn new(picture: Solution, clues: DynPuzzle, current_color: Color) -> Self {
        SolveGui {
            canvas: CanvasGui {
                picture,
                current_color,
                drag_start_color: current_color,
                undo_stack: vec![],
                redo_stack: vec![],
                current_tool: Tool::Pencil,
                line_tool_state: None,
            },
            clues,
        }
    }

    fn palette(&mut self, ui: &mut egui::Ui) {
        use itertools::Itertools;

        let mut picked_color = self.canvas.current_color;

        for (color, color_info) in self
            .canvas
            .picture
            .palette
            .iter()
            .sorted_by_key(|(color, _)| *color)
        {
            if *color == UNSOLVED {
                continue;
            }
            let (r, g, b) = color_info.rgb;
            let button_text = if color_info.corner.is_some() {
                color_info.ch.to_string()
            } else {
                "■".to_string()
            };

            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(icons::ICON_CHEVRON_RIGHT)
                        .size(24.0)
                        .color(Color32::from_black_alpha(
                            if *color == picked_color { 255 } else { 0 },
                        )),
                );

                let color_text = RichText::new(button_text)
                    .monospace()
                    .size(24.0)
                    .color(egui::Color32::from_rgb(r, g, b));
                if ui
                    .add_enabled(*color != picked_color, egui::Button::new(color_text))
                    .clicked()
                {
                    picked_color = *color;
                };
            });
        }
        self.canvas.current_color = picked_color;
    }

    fn tool_selector(&mut self, ui: &mut egui::Ui) {
        ui.label("Tools");
        ui.horizontal(|ui| {
            ui.selectable_value(
                &mut self.canvas.current_tool,
                Tool::Pencil,
                egui::RichText::new(icons::ICON_BRUSH).size(24.0),
            )
            .on_hover_text("Pencil");
            ui.selectable_value(
                &mut self.canvas.current_tool,
                Tool::OrthographicLine,
                egui::RichText::new(icons::ICON_LINE_START).size(24.0),
            )
            .on_hover_text("Orthographic line");
            ui.selectable_value(
                &mut self.canvas.current_tool,
                Tool::FloodFill,
                egui::RichText::new(icons::ICON_FORMAT_COLOR_FILL).size(24.0),
            )
            .on_hover_text("Flood Fill");
        });
    }

    pub fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.vertical(|ui| {
            ui.set_width(120.0);
            ui.horizontal(|ui| {
                ui.label(format!("({})", self.canvas.undo_stack.len()));
                if ui.button(icons::ICON_UNDO).clicked()
                    || ui.input(|i| i.key_pressed(egui::Key::Z))
                {
                    self.canvas.un_or_re_do(true);
                }
                if ui.button(icons::ICON_REDO).clicked()
                    || ui.input(|i| i.key_pressed(egui::Key::Y))
                {
                    self.canvas.un_or_re_do(false);
                }
                ui.label(format!("({})", self.canvas.redo_stack.len()));
            });

            ui.separator();

            self.tool_selector(ui);

            ui.separator();

            self.palette(ui);
        });
    }

    pub fn canvas(&mut self, ui: &mut egui::Ui, scale: f32) {
        let x_size = self.canvas.picture.grid.len();
        let y_size = self.canvas.picture.grid.first().unwrap().len();

        let (response, painter) = ui.allocate_painter(
            Vec2::new(scale * x_size as f32, scale * y_size as f32) + Vec2::new(2.0, 2.0), // for the border
            egui::Sense::click_and_drag(),
        );

        let canvas_without_border = response.rect.shrink(1.0);

        let to_screen = egui::emath::RectTransform::from_to(
            Rect::from_min_size(Pos2::ZERO, Vec2::new(x_size as f32, y_size as f32)),
            canvas_without_border,
        );
        let from_screen = to_screen.inverse();

        if let Some(pointer_pos) = response.interact_pointer_pos() {
            let canvas_pos = from_screen * pointer_pos;
            let x = canvas_pos.x as usize;
            let y = canvas_pos.y as usize;

            if (0..x_size).contains(&x) && (0..y_size).contains(&y) {
                match self.canvas.current_tool {
                    Tool::Pencil => {
                        if response.clicked() || response.dragged() {
                            let new_color = if self.canvas.picture.grid[x][y]
                                == self.canvas.current_color
                            {
                                UNSOLVED
                            } else {
                                self.canvas.current_color
                            };
                            let mood = if response.clicked() || response.drag_started() {
                                self.canvas.drag_start_color = new_color;
                                ActionMood::Normal
                            } else {
                                ActionMood::Merge
                            };

                            let mut changes = HashMap::new();
                            changes.insert((x, y), self.canvas.drag_start_color);
                            self.canvas
                                .perform(Action::ChangeColor { changes }, mood);
                        }
                    }
                    Tool::FloodFill => {
                        if response.clicked() {
                            self.canvas.flood_fill(x, y);
                        }
                    }
                    Tool::OrthographicLine => {
                        if response.clicked() || response.drag_started() {
                            let new_color = if self.canvas.picture.grid[x][y]
                                == self.canvas.current_color
                            {
                                UNSOLVED
                            } else {
                                self.canvas.current_color
                            };
                            self.canvas.drag_start_color = new_color;

                            self.canvas.line_tool_state = Some((x, y));

                            self.canvas.perform(
                                Action::ChangeColor {
                                    changes: [((x, y), self.canvas.drag_start_color)].into(),
                                },
                                ActionMood::Normal,
                            );
                        } else if response.dragged() {
                            if let Some((start_x, start_y)) = self.canvas.line_tool_state {
                                let mut new_points = HashMap::new();

                                let horiz = x.abs_diff(start_x) > y.abs_diff(start_y);

                                if horiz {
                                    let xlo = min(start_x, x);
                                    let xhi = max(start_x, x);
                                    for xi in xlo..=xhi {
                                        new_points
                                            .insert((xi, start_y), self.canvas.drag_start_color);
                                    }
                                } else {
                                    let ylo = min(start_y, y);
                                    let yhi = max(start_y, y);
                                    for yi in ylo..=yhi {
                                        new_points
                                            .insert((start_x, yi), self.canvas.drag_start_color);
                                    }
                                }
                                self.canvas.perform(
                                    Action::ChangeColor {
                                        changes: new_points,
                                    },
                                    ActionMood::ReplaceAction,
                                );
                            }
                        } else if response.drag_stopped() {
                            self.canvas.line_tool_state = None;
                        }
                    }
                }
            }
        }

        let mut shapes = vec![];

        for y in 0..y_size {
            for x in 0..x_size {
                let cell = self.canvas.picture.grid[x][y];
                let color_info = &self.canvas.picture.palette[&cell];

                for shape in cell_shape(
                    color_info,
                    true,
                    (&self.canvas.picture.palette[&BACKGROUND], 1.0),
                    x,
                    y,
                    &to_screen,
                ) {
                    shapes.push(shape);
                }
            }
        }

        // Grid lines:
        for y in 0..=y_size {
            let points = [
                to_screen * Pos2::new(0.0, y as f32),
                to_screen * Pos2::new(x_size as f32, y as f32),
            ];
            let stroke = egui::Stroke::new(
                1.0,
                egui::Color32::from_black_alpha(if y % 5 == 0 { 64 } else { 16 }),
            );
            shapes.push(egui::Shape::line_segment(points, stroke));
        }
        for x in 0..=x_size {
            let points = [
                to_screen * Pos2::new(x as f32, 0.0),
                to_screen * Pos2::new(x as f32, y_size as f32),
            ];
            let stroke = egui::Stroke::new(
                1.0,
                egui::Color32::from_black_alpha(if x % 5 == 0 { 64 } else { 16 }),
            );
            shapes.push(egui::Shape::line_segment(points, stroke));
        }

        painter.extend(shapes);
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

pub fn draw_dyn_col_clues(ui: &mut egui::Ui, puzzle: &DynPuzzle, scale: f32) {
    match puzzle {
        DynPuzzle::Nono(puzzle) => {
            draw_col_clues::<crate::puzzle::Nono>(ui, puzzle, scale);
        }
        DynPuzzle::Triano(puzzle) => {
            draw_col_clues::<crate::puzzle::Triano>(ui, puzzle, scale);
        }
    }
}

fn draw_col_clues<C: crate::puzzle::Clue>(
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

    let mut max_height: f32 = 0.0;
    for col in &puzzle.cols {
        let mut this_height = 0.0;
        for clue in col {
            this_height += box_side * (clue.express(puzzle).len() as f32) + between_clues;
        }
        max_height = max_height.max(this_height);
    }
    max_height += puzz_padding;

    let (response, painter) = ui.allocate_painter(
        Vec2::new(scale * puzzle.cols.len() as f32, max_height) + Vec2::new(2.0, 2.0),
        egui::Sense::empty(),
    );

    for x in 0..puzzle.cols.len() {
        let col_clues = &puzzle.cols[x];
        let mut current_y = response.rect.max.y - puzz_padding;

        for clue in col_clues.iter().rev() {
            let expressed_clues = clue.express(puzzle);

            for (color_info, len) in expressed_clues.into_iter().rev() {
                let (r, g, b) = color_info.rgb;
                let bg_color = egui::Color32::from_rgb(r, g, b);

                let corner_b_r = Pos2::new(
                    response.rect.min.x + (x as f32 + 1.0) * scale - box_margin,
                    current_y,
                );

                if let Some(len) = len {
                    assert!(len > 0);

                    let clue_txt = len.to_string();
                    let clue_font = fonts_by_digit[clue_txt.len()].clone();

                    let corner_u_l = corner_b_r + Vec2::new(-box_side, -box_side);

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
                    current_y -= box_side;
                } else {
                    let mut triangle = crate::gui::triangle_shape(
                        color_info.corner.expect("must be a corner"),
                        bg_color,
                        Vec2::new(box_side, box_side),
                    );
                    let corner_u_l = corner_b_r + Vec2::new(-box_side, -box_side);
                    triangle.translate(corner_u_l.to_vec2());
                    current_y -= box_side;

                    painter.add(triangle);
                }
            }
            current_y -= between_clues;
        }
    }
}
