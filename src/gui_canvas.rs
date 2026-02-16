use std::{
    cmp::{max, min},
    collections::HashMap,
};

use crate::{
    gui_solver::RenderStyle,
    puzzle::{
        BACKGROUND, Color, ColorInfo, Corner, Document, UNSOLVED,
    },
};
use egui::{Color32, Pos2, Rect, RichText, Shape, Vec2};
use egui_material_icons::icons;

use crate::gui::Disambiguator;

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum Tool {
    Pencil,
    FloodFill,
    OrthographicLine,
}

pub type Version = u32;

pub struct Staleable<T> {
    pub val: T,
    pub version: Version,
}

impl<T> Staleable<T> {
    pub fn update(&mut self, val: T, version: Version) {
        self.val = val;
        self.version = version;
    }

    pub fn fresh(&self, version: Version) -> bool {
        self.version == version
    }

    pub fn get_if_fresh(&self, version: Version) -> Option<&T> {
        if self.fresh(version) {
            Some(&self.val)
        } else {
            None
        }
    }

    pub fn get_or_refresh<'a, F>(&'a mut self, version: Version, refresh: F) -> &'a mut T
    where
        F: FnOnce() -> T,
    {
        if !self.fresh(version) {
            self.val = refresh();
            self.version = version;
        }
        &mut self.val
    }
}

pub struct CanvasGui {
    pub document: Document,
    pub version: Version,
    pub current_color: Color,
    pub drag_start_color: Color,
    pub undo_stack: Vec<Action>,
    pub redo_stack: Vec<Action>,
    pub current_tool: Tool,
    pub line_tool_state: Option<(usize, usize)>,
    pub solved_mask: Staleable<(String, Vec<Vec<bool>>)>,
    pub disambiguator: Staleable<Disambiguator>,
    pub id: Staleable<String>,
}

#[derive(Clone, Debug)]
pub enum Action {
    ChangeColor {
        changes: HashMap<(usize, usize), Color>,
    },
    ReplaceDocument {
        document: Document,
    },
}

#[derive(PartialEq, Eq)]
pub enum ActionMood {
    Normal,
    Merge,
    ReplaceAction,
    Undo,
    Redo,
}

impl CanvasGui {
    fn reversed(&self, action: &Action) -> Action {
        match action {
            Action::ChangeColor { changes } => Action::ChangeColor {
                changes: changes
                    .keys()
                    .map(|(x, y)| ((*x, *y), self.document.try_solution().unwrap().grid[*x][*y]))
                    .collect::<HashMap<_, _>>(),
            },
            Action::ReplaceDocument { document: _ } => Action::ReplaceDocument {
                document: self.document.clone(),
            },
        }
    }

    pub fn perform(&mut self, action: Action, mood: ActionMood) {
        use Action::*;
        use ActionMood::*;

        let mood = if mood == Merge || mood == ReplaceAction {
            match (self.undo_stack.last_mut(), &action) {
                // Consecutive `ChangeColor`s can be merged with each other.
                (
                    Some(ChangeColor { changes }),
                    ChangeColor {
                        changes: new_changes,
                    },
                ) => {
                    let picture = self.document.solution_mut();
                    if mood == ReplaceAction {
                        for ((x, y), _) in new_changes {
                            changes.entry((*x, *y)).or_insert(picture.grid[*x][*y]);
                        }
                        changes.retain(|(x, y), old_col| {
                            if !new_changes.contains_key(&(*x, *y)) {
                                picture.grid[*x][*y] = *old_col;
                                self.version += 1;
                                false
                            } else {
                                true
                            }
                        });
                        for ((x, y), col) in new_changes {
                            if picture.grid[*x][*y] != *col {
                                picture.grid[*x][*y] = *col;
                                self.version += 1;
                            }
                        }
                        return;
                    } else {
                        for ((x, y), col) in new_changes {
                            if !changes.contains_key(&(*x, *y)) {
                                changes.insert((*x, *y), picture.grid[*x][*y]);
                                // Crucially, this only fires on a new cell!
                                // Otherwise, we'd be flipping cells back and forth as long as we
                                // were in them!
                                picture.grid[*x][*y] = *col;
                                self.version += 1;
                            }
                        }
                        return;
                    }
                }
                _ => Normal, // Unable to merge; add a new undo entry.
            }
        } else {
            mood
        };

        let reversed_action = self.reversed(&action);

        match action {
            Action::ChangeColor { changes } => {
                let picture = self.document.solution_mut();
                for ((x, y), new_color) in changes {
                    if picture.grid[x][y] != new_color {
                        picture.grid[x][y] = new_color;
                        self.version += 1;
                    }
                }
            }
            Action::ReplaceDocument { document } => {
                self.document = document;
                self.version += 1;
            }
        }

        match mood {
            Merge | ReplaceAction => {}
            Normal => {
                self.undo_stack.push(reversed_action);
                self.redo_stack.clear();
            }
            Undo => {
                self.redo_stack.push(reversed_action);
            }
            Redo => {
                self.undo_stack.push(reversed_action);
            }
        }
    }

    pub fn un_or_re_do(&mut self, un: bool) {
        let action = if un {
            self.undo_stack.pop()
        } else {
            self.redo_stack.pop()
        };

        if let Some(action) = action {
            self.perform(
                action,
                if un {
                    ActionMood::Undo
                } else {
                    ActionMood::Redo
                },
            )
        }
    }

    pub fn common_sidebar_items(&mut self, ui: &mut egui::Ui, palette_read_only: bool) {
        ui.horizontal(|ui| {
            ui.label(format!("({})", self.undo_stack.len()));
            if ui.button(icons::ICON_UNDO).clicked() || ui.input(|i| i.key_pressed(egui::Key::Z)) {
                self.un_or_re_do(true);
            }
            if ui.button(icons::ICON_REDO).clicked() || ui.input(|i| i.key_pressed(egui::Key::Y)) {
                self.un_or_re_do(false);
            }
            ui.label(format!("({})", self.redo_stack.len()));
        });

        ui.separator();

        self.tool_selector(ui);

        ui.separator();

        self.palette_editor(ui, palette_read_only);
    }

    fn tool_selector(&mut self, ui: &mut egui::Ui) {
        ui.label("Tools");
        ui.horizontal(|ui| {
            ui.selectable_value(
                &mut self.current_tool,
                Tool::Pencil,
                egui::RichText::new(icons::ICON_BRUSH).size(24.0),
            )
            .on_hover_text("Pencil");
            ui.selectable_value(
                &mut self.current_tool,
                Tool::OrthographicLine,
                egui::RichText::new(icons::ICON_LINE_START).size(24.0),
            )
            .on_hover_text("Orthographic line");
            ui.selectable_value(
                &mut self.current_tool,
                Tool::FloodFill,
                egui::RichText::new(icons::ICON_FORMAT_COLOR_FILL).size(24.0),
            )
            .on_hover_text("Flood Fill");
        });
    }

    fn flood_fill(&mut self, x: usize, y: usize) {
        let picture = self.document.solution_mut();
        let target_color = picture.grid[x][y];
        if target_color == self.current_color {
            return; // Nothing to do
        }

        let mut changes = HashMap::new();
        let mut q = std::collections::VecDeque::new();

        q.push_back((x, y));
        let mut visited = std::collections::HashSet::new();
        visited.insert((x, y));

        let x_size = picture.grid.len();
        let y_size = picture.grid.first().unwrap().len();

        while let Some((px, py)) = q.pop_front() {
            changes.insert((px, py), self.current_color);

            let neighbors = [
                (px.wrapping_sub(1), py),
                (px + 1, py),
                (px, py.wrapping_sub(1)),
                (px, py + 1),
            ];

            for (nx, ny) in neighbors {
                if nx < x_size && ny < y_size && picture.grid[nx][ny] == target_color {
                    if visited.insert((nx, ny)) {
                        q.push_back((nx, ny));
                    }
                }
            }
        }

        if !changes.is_empty() {
            self.perform(Action::ChangeColor { changes }, ActionMood::Normal);
        }
    }

    pub fn canvas(
        &mut self,
        ui: &mut egui::Ui,
        scale: f32,
        render_style: RenderStyle,
    ) -> Option<(usize, usize)> {
        let picture = self.document.solution_mut();
        let x_size = picture.grid.len();
        let y_size = picture.grid.first().unwrap().len();

        let (mut response, painter) = ui.allocate_painter(
            Vec2::new(scale * x_size as f32, scale * y_size as f32) + Vec2::new(2.0, 2.0), // for the border
            egui::Sense::click_and_drag(),
        );

        let canvas_without_border = response.rect.shrink(1.0);

        let to_screen = egui::emath::RectTransform::from_to(
            Rect::from_min_size(Pos2::ZERO, Vec2::new(x_size as f32, y_size as f32)),
            canvas_without_border,
        );
        let from_screen = to_screen.inverse();

        let mut hovered_cell = None;
        if let Some(pointer_pos) = response.hover_pos() {
            let canvas_pos = from_screen * pointer_pos;
            let x = canvas_pos.x as usize;
            let y = canvas_pos.y as usize;
            if (0..x_size).contains(&x) && (0..y_size).contains(&y) {
                hovered_cell = Some((x, y));
            }
        }

        if let Some(pointer_pos) = response.interact_pointer_pos() {
            let canvas_pos = from_screen * pointer_pos;
            let x = canvas_pos.x as usize;
            let y = canvas_pos.y as usize;

            if (0..x_size).contains(&x) && (0..y_size).contains(&y) {
                let pointer = &ui.input(|i| i.pointer.clone());
                let paint_color = if pointer.middle_down() {
                    if self.document.solution_mut().palette.contains_key(&UNSOLVED) {
                        UNSOLVED
                    } else {
                        BACKGROUND
                    }
                } else if pointer.secondary_down() {
                    BACKGROUND
                } else if picture.grid[x][y] != self.current_color {
                    self.current_color
                } else {
                    BACKGROUND
                };

                match self.current_tool {
                    Tool::Pencil => {
                        let mood = if pointer.any_pressed() {
                            self.drag_start_color = paint_color;
                            ActionMood::Normal
                        } else {
                            ActionMood::Merge
                        };

                        let mut changes = HashMap::new();
                        changes.insert((x, y), self.drag_start_color);
                        self.perform(Action::ChangeColor { changes }, mood);
                    }
                    Tool::FloodFill => {
                        if pointer.any_click() {
                            let original_color = self.current_color;
                            self.current_color = paint_color;
                            self.flood_fill(x, y);
                            self.current_color = original_color;
                        }
                    }
                    Tool::OrthographicLine => {
                        if pointer.any_pressed() {
                            self.drag_start_color = paint_color;

                            self.line_tool_state = Some((x, y));

                            self.perform(
                                Action::ChangeColor {
                                    changes: [((x, y), self.drag_start_color)].into(),
                                },
                                ActionMood::Normal,
                            );
                        } else if pointer.any_down() {
                            if let Some((start_x, start_y)) = self.line_tool_state {
                                let mut new_points = HashMap::new();

                                let horiz = x.abs_diff(start_x) > y.abs_diff(start_y);

                                if horiz {
                                    let xlo = min(start_x, x);
                                    let xhi = max(start_x, x);
                                    for xi in xlo..=xhi {
                                        new_points.insert((xi, start_y), self.drag_start_color);
                                    }
                                } else {
                                    let ylo = min(start_y, y);
                                    let yhi = max(start_y, y);
                                    for yi in ylo..=yhi {
                                        new_points.insert((start_x, yi), self.drag_start_color);
                                    }
                                }
                                self.perform(
                                    Action::ChangeColor {
                                        changes: new_points,
                                    },
                                    ActionMood::ReplaceAction,
                                );
                            }
                        } else if pointer.any_released() {
                            self.line_tool_state = None;
                        }
                    }
                }
            }
        }

        let mut shapes = vec![];
        let disambiguator = self.disambiguator.get_if_fresh(self.version);
        let disambig_report = disambiguator.as_ref().and_then(|d| d.report.as_ref());

        let picture = self.document.try_solution().unwrap();
        for y in 0..y_size {
            for x in 0..x_size {
                let cell = picture.grid[x][y];
                let color_info = &picture.palette[&cell];
                let solved = self
                    .solved_mask
                    .get_if_fresh(self.version)
                    .map_or(true, |sm| sm.1[x][y])
                    || disambig_report.is_some()
                    || disambiguator.map_or(false, |d| d.progress > 0.0 && d.progress < 1.0);
                let mut dr = (&picture.palette[&BACKGROUND], 1.0);

                if let Some(disambig_report) = disambig_report.as_ref() {
                    let (c, score) = disambig_report[x][y];
                    dr = (&picture.palette[&c], score);
                }
                for shape in cell_shape(color_info, solved, dr, x, y, &to_screen, render_style) {
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
        response.mark_changed();

        hovered_cell
    }

    fn palette_editor(&mut self, ui: &mut egui::Ui, read_only: bool) {
        let mut picked_color = self.current_color;
        let mut removed_color = None;
        let mut add_color = false;

        use itertools::Itertools;

        for (color, color_info) in self
            .document
            .solution_mut()
            .palette
            .iter_mut()
            .sorted_by_key(|(color, _)| *color)
        {
            // TODO: actually paint a palette entry for unsolved,
            // in case the user doesn't have a middle button.
            if *color == UNSOLVED && read_only {
                continue;
            }
            let (r, g, b) = color_info.rgb;
            let button_text = if color_info.corner.is_some() {
                color_info.ch.to_string()
            } else {
                "■".to_string()
            };

            ui.horizontal(|ui| {
                ui.label(RichText::new(icons::ICON_CHEVRON_FORWARD).size(24.0).color(
                    Color32::from_black_alpha(if *color == picked_color { 255 } else { 0 }),
                ));

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

                if !read_only {
                    let mut edited_color = [r as f32 / 256.0, g as f32 / 256.0, b as f32 / 256.0];

                    if ui.color_edit_button_rgb(&mut edited_color).changed() {
                        // TODO: this should probably also be undoable
                        picked_color = *color;
                        color_info.rgb = (
                            (edited_color[0] * 256.0) as u8,
                            (edited_color[1] * 256.0) as u8,
                            (edited_color[2] * 256.0) as u8,
                        );
                    }
                    if *color != BACKGROUND {
                        if ui.button(icons::ICON_DELETE).clicked() {
                            removed_color = Some(*color);
                        }
                    }
                }
            });
        }
        if !read_only && ui.button("New color").clicked() {
            add_color = true;
        }
        self.current_color = picked_color;

        if Some(self.current_color) == removed_color {
            self.current_color = BACKGROUND;
        }

        if let Some(removed_color) = removed_color {
            let mut new_document = self.document.clone();
            let new_picture = new_document.solution_mut();
            for row in new_picture.grid.iter_mut() {
                for cell in row.iter_mut() {
                    if *cell == removed_color {
                        *cell = self.current_color;
                    }
                }
            }
            new_picture.palette.remove(&removed_color);
            self.perform(
                Action::ReplaceDocument {
                    document: new_document,
                },
                ActionMood::Normal,
            );
        }
        if add_color {
            let mut new_document = self.document.clone();
            let new_picture = new_document.solution_mut();
            let next_color = Color(new_picture.palette.keys().map(|k| k.0).max().unwrap() + 1);
            new_picture.palette.insert(
                next_color,
                ColorInfo {
                    ch: (next_color.0 + 65) as char, // TODO: will break chargrid export
                    name: "New color".to_string(),
                    rgb: (128, 128, 128),
                    color: next_color,
                    corner: None,
                },
            );
            self.perform(
                Action::ReplaceDocument {
                    document: new_document,
                },
                ActionMood::Normal,
            );
        }
    }
}

pub fn triangle_shape(corner: Corner, color: egui::Color32, scale: Vec2) -> egui::Shape {
    let Corner { left, upper } = corner;

    let mut points = vec![];
    // The `+`ed offsets are empirircally-set to make things fit better.
    if left || upper {
        points.push((Vec2::new(0.0, 0.0) * scale + Vec2::new(0.25, -0.5)).to_pos2());
    }
    if !left || upper {
        points.push((Vec2::new(1.0, 0.0) * scale + Vec2::new(0.25, -0.5)).to_pos2());
    }
    if !left || !upper {
        points.push((Vec2::new(1.0, 1.0) * scale + Vec2::new(0.25, 0.5)).to_pos2());
    }
    if left || !upper {
        points.push((Vec2::new(0.0, 1.0) * scale + Vec2::new(0.25, 0.5)).to_pos2());
    }

    Shape::convex_polygon(points, color, (0.0, color))
}

fn cell_shape(
    ci: &ColorInfo,
    solved: bool,
    disambig: (&ColorInfo, f32),
    x: usize,
    y: usize,
    to_screen: &egui::emath::RectTransform,
    render_style: RenderStyle,
) -> Vec<egui::Shape> {
    let (r, g, b) = ci.rgb;
    let color = if ci.color == UNSOLVED {
        if render_style == RenderStyle::Experimental {
            egui::Color32::from_rgb(160, 160, 160)
        } else {
            egui::Color32::WHITE
        }
    } else {
        egui::Color32::from_rgb(r, g, b)
    };

    let mut actual_cell = match ci.corner {
        None => egui::Shape::rect_filled(
            Rect::from_min_size(Pos2::new(0.3, 0.0), to_screen.scale()),
            0.0,
            color,
        ),
        Some(corner) => triangle_shape(corner, color, to_screen.scale()),
    };

    actual_cell.translate((to_screen * Pos2::new(x as f32, y as f32)).to_vec2());

    let mut res = vec![actual_cell];

    if ci.color == BACKGROUND {
        let center = to_screen * Pos2::new(x as f32 + 0.5, y as f32 + 0.5);
        match render_style {
            RenderStyle::TraditionalDots => {
                res.push(egui::Shape::circle_filled(
                    center,
                    to_screen.scale().x * 0.1,
                    egui::Color32::from_rgb(190, 190, 190),
                ));
            }
            RenderStyle::TraditionalXes => {
                let stroke = egui::Stroke::new(2.0, Color32::from_rgb(190, 190, 190));
                let radius = to_screen.scale().x * 0.2;
                res.push(egui::Shape::line_segment(
                    [
                        center + Vec2::new(-radius, -radius),
                        center + Vec2::new(radius, radius),
                    ],
                    stroke,
                ));
                res.push(egui::Shape::line_segment(
                    [
                        center + Vec2::new(radius, -radius),
                        center + Vec2::new(-radius, radius),
                    ],
                    stroke,
                ));
            }
            RenderStyle::Experimental => {}
        }
    }

    if ci.color == UNSOLVED && render_style == RenderStyle::Experimental {
        res.push(egui::Shape::convex_polygon(
            vec![
                to_screen * Pos2::new(x as f32 + 0.5, y as f32 + 0.0),
                to_screen * Pos2::new(x as f32 + 1.0, y as f32 + 0.5),
                to_screen * Pos2::new(x as f32 + 0.5, y as f32 + 1.0),
                to_screen * Pos2::new(x as f32 + 0.0, y as f32 + 0.5),
            ],
            egui::Color32::from_rgb(230, 230, 230),
            egui::Stroke::default(),
        ));
    }

    if !solved {
        res.push(egui::Shape::circle_filled(
            to_screen * Pos2::new(x as f32 + 0.5, y as f32 + 0.5),
            to_screen.scale().x * 0.3,
            egui::Color32::from_rgb(190, 190, 190),
        ))
    }

    if disambig.1 < 1.0 {
        let (r, g, b) = disambig.0.rgb;
        res.push(egui::Shape::rect_filled(
            Rect::from_min_size(
                to_screen * Pos2::new(x as f32 + 0.25, y as f32 + 0.25),
                to_screen.scale() * 0.5,
            ),
            0.0,
            Color32::from_rgba_unmultiplied(r, g, b, ((1.0 - disambig.1) * 255.0) as u8),
        ));
    }

    res
}
