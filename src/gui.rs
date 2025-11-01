use std::{
    cmp::{max, min},
    collections::HashMap,
    sync::mpsc,
};

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum Tool {
    Pencil,
    FloodFill,
    OrthographicLine,
}

use crate::{
    export::to_bytes,
    grid_solve::{self, disambig_candidates},
    gui_solver::{RenderStyle, SolveGui},
    import,
    puzzle::{
        BACKGROUND, ClueStyle, Color, ColorInfo, Corner, Document, PuzzleDynOps, Solution, UNSOLVED,
    },
    user_settings::{UserSettings, consts},
};
use egui::{Color32, Pos2, Rect, RichText, Shape, Style, Vec2, Visuals};
use egui_material_icons::icons;

#[cfg(not(target_arch = "wasm32"))]
pub fn edit_image(document: Document) {
    let native_options = eframe::NativeOptions::default();
    eframe::run_native(
        "Number Loom",
        native_options,
        Box::new(|cc| {
            egui_material_icons::initialize(&cc.egui_ctx);
            Ok(Box::new(NonogramGui::new(document)))
        }),
    )
    .unwrap()
}

#[cfg(target_arch = "wasm32")]
pub fn edit_image(document: Document) {
    use eframe::wasm_bindgen::JsCast as _;

    let web_options = eframe::WebOptions::default();

    wasm_bindgen_futures::spawn_local(async {
        let sys_doc = web_sys::window()
            .expect("No window")
            .document()
            .expect("No document");

        let canvas = sys_doc
            .get_element_by_id("the_canvas_id")
            .expect("Failed to find the_canvas_id")
            .dyn_into::<web_sys::HtmlCanvasElement>()
            .expect("the_canvas_id was not a HtmlCanvasElement");

        let start_result = eframe::WebRunner::new()
            .start(
                canvas,
                web_options,
                Box::new(|cc| {
                    egui_material_icons::initialize(&cc.egui_ctx);
                    Ok(Box::new(NonogramGui::new(document)))
                }),
            )
            .await;

        // Remove the loading text and spinner:
        if let Some(loading_text) = sys_doc.get_element_by_id("loading_text") {
            match start_result {
                Ok(_) => {
                    loading_text.remove();
                }
                Err(e) => {
                    panic!("Failed to start eframe: {:?}", e);
                }
            }
        }
    });
}

#[cfg(target_arch = "wasm32")]
use wasm_bindgen_futures::spawn_local as spawn_async;

#[cfg(not(target_arch = "wasm32"))]
pub fn spawn_async<F>(future: F)
where
    F: std::future::Future<Output = ()> + 'static + std::marker::Send,
{
    // This sort of weird construct allows us to avoid multithreaded tokio,
    // which isn't available on wasm32 (cargo doesn't like having the same crate have different
    // features on different platforms, and we might want to use some tokio features on wasm32)
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(future);
    });
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn yield_now() {
    tokio::task::yield_now().await;
}

#[cfg(target_arch = "wasm32")]
pub async fn yield_now() {
    // Taken from https://github.com/rustwasm/wasm-bindgen/issues/3359:
    let mut cb = |resolve: js_sys::Function, _reject: js_sys::Function| {
        web_sys::window()
            .unwrap()
            .set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, 1)
            .expect("Failed to call set_timeout");
    };
    let p = js_sys::Promise::new(&mut cb);
    wasm_bindgen_futures::JsFuture::from(p).await.unwrap();
}

type Version = u32;

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

    fn get_if_fresh(&self, version: Version) -> Option<&T> {
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
}

pub struct NonogramGui {
    editor_gui: CanvasGui,
    scale: f32,
    opened_file_receiver: mpsc::Receiver<Document>,
    library_receiver: mpsc::Receiver<Vec<Document>>,
    library_dialog: Option<Vec<Document>>,
    new_dialog: Option<NewPuzzleDialog>,
    auto_solve: bool,
    lines_to_affect_string: String,
    solve_report: String,
    solve_mode: bool,
    solve_gui: Option<SolveGui>,
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

impl NonogramGui {
    pub fn new(mut document: Document) -> Self {
        // (Public for testing)
        let picture = document.try_solution().unwrap();
        let solved_mask = vec![vec![true; picture.grid[0].len()]; picture.grid.len()];

        let mut current_color = BACKGROUND;
        if picture.palette.contains_key(&Color(1)) {
            current_color = Color(1);
        }

        if document.author.is_empty() {
            if let Some(author) = UserSettings::get(consts::EDITOR_AUTHOR_NAME) {
                document.author = author;
            }
        }

        NonogramGui {
            editor_gui: CanvasGui {
                document,
                version: 0,
                current_color,
                drag_start_color: current_color,
                undo_stack: vec![],
                redo_stack: vec![],
                current_tool: Tool::Pencil,
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
            scale: 16.0,
            opened_file_receiver: mpsc::channel().1,
            library_receiver: mpsc::channel().1,
            new_dialog: None,
            library_dialog: None,
            auto_solve: false,
            lines_to_affect_string: "5".to_string(),
            solve_report: "".to_string(),
            solve_mode: false,
            solve_gui: None,
        }
    }

    fn resize(&mut self, top: Option<bool>, left: Option<bool>, add: bool) {
        let picture = self.editor_gui.document.solution_mut();
        let mut g = picture.grid.clone();
        let lines = match self.lines_to_affect_string.parse::<usize>() {
            Ok(lines) => lines,
            Err(_) => {
                self.lines_to_affect_string += "??";
                return;
            }
        };
        if let Some(left) = left {
            if add {
                g.resize(g.len() + lines, vec![BACKGROUND; g.first().unwrap().len()]);
                if left {
                    g.rotate_right(lines);
                }
            } else {
                if left {
                    g.rotate_left(lines);
                }
                g.truncate(g.len() - lines);
            }
        } else if let Some(top) = top {
            if add {
                for row in g.iter_mut() {
                    row.resize(row.len() + lines, BACKGROUND);
                    if top {
                        row.rotate_right(lines);
                    }
                }
            } else {
                for row in g.iter_mut() {
                    if top {
                        row.rotate_left(lines);
                    }
                    row.truncate(row.len() - lines);
                }
            }
        }

        let mut new_doc = self.editor_gui.document.clone();
        new_doc.solution_mut().grid = g;
        self.editor_gui.perform(
            Action::ReplaceDocument { document: new_doc },
            ActionMood::Normal,
        );
    }

    fn resizer(&mut self, ui: &mut egui::Ui) {
        let picture = self.editor_gui.document.try_solution().unwrap();
        ui.label(format!(
            "Canvas size: {}x{}",
            picture.x_size(),
            picture.y_size(),
        ));

        egui::Grid::new("resizer").show(ui, |ui| {
            ui.label("");
            ui.horizontal(|ui| {
                if ui.button(icons::ICON_ADD).clicked() {
                    self.resize(Some(true), None, true);
                }
                if ui.button(icons::ICON_REMOVE).clicked() {
                    self.resize(Some(true), None, false);
                }
            });
            ui.label("");
            ui.end_row();

            ui.vertical(|ui| {
                if ui.button(icons::ICON_ADD).clicked() {
                    self.resize(None, Some(true), true);
                }
                if ui.button(icons::ICON_REMOVE).clicked() {
                    self.resize(None, Some(true), false);
                }
            });
            ui.text_edit_singleline(&mut self.lines_to_affect_string);

            ui.vertical(|ui| {
                if ui.button(icons::ICON_ADD).clicked() {
                    self.resize(None, Some(false), true);
                }
                if ui.button(icons::ICON_REMOVE).clicked() {
                    self.resize(None, Some(false), false);
                }
            });
            ui.end_row();

            ui.label("");
            ui.horizontal(|ui| {
                if ui.button(icons::ICON_ADD).clicked() {
                    self.resize(Some(false), None, true);
                }
                if ui.button(icons::ICON_REMOVE).clicked() {
                    self.resize(Some(false), None, false);
                }
            });
            ui.label("");
        });
    }

    fn edit_sidebar(&mut self, ui: &mut egui::Ui) {
        ui.vertical(|ui| {
            ui.set_width(140.0);
            ui.add(
                egui::TextEdit::singleline(&mut self.editor_gui.document.title).hint_text("Title"),
            );

            ui.horizontal(|ui| {
                ui.label("by ");
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut self.editor_gui.document.author)
                            .hint_text("Author"),
                    )
                    .changed()
                {
                    let _ = UserSettings::set(
                        consts::EDITOR_AUTHOR_NAME,
                        &self.editor_gui.document.author,
                    );
                }
            });

            self.editor_gui.common_sidebar_items(ui, false);

            ui.separator();

            self.resizer(ui);

            ui.separator();
            ui.checkbox(&mut self.auto_solve, "auto-solve");
            if ui.button("Solve").clicked() || self.auto_solve {
                let puzzle = self.editor_gui.document.try_solution().unwrap().to_puzzle();

                let (report, _solved_mask) =
                    self.editor_gui
                        .solved_mask
                        .get_or_refresh(self.editor_gui.version, || match puzzle.plain_solve() {
                            Ok(grid_solve::Report {
                                solve_counts,
                                cells_left,
                                solution: _solution,
                                solved_mask,
                            }) => (
                                format!("{solve_counts} unsolved cells: {cells_left}"),
                                solved_mask,
                            ),
                            Err(e) => (format!("Error: {:?}", e), vec![]),
                        });
                self.solve_report = report.clone();
            }

            ui.colored_label(
                if self.editor_gui.solved_mask.fresh(self.editor_gui.version) {
                    Color32::BLACK
                } else {
                    Color32::GRAY
                },
                &self.solve_report,
            );

            ui.separator();

            self.editor_gui
                .disambiguator
                .get_or_refresh(self.editor_gui.version, Disambiguator::new)
                .disambig_widget(self.editor_gui.document.try_solution().unwrap(), ui);

            ui.label("Description:");
            ui.text_edit_multiline(&mut self.editor_gui.document.description);
        });
    }

    fn loader(&mut self, ui: &mut egui::Ui) {
        if ui.button("Open").clicked() {
            let (sender, receiver) = mpsc::channel();
            self.opened_file_receiver = receiver;

            spawn_async(async move {
                let handle = rfd::AsyncFileDialog::new()
                    .add_filter(
                        "all recognized formats",
                        &["png", "gif", "bmp", "xml", "pbn", "txt", "g"],
                    )
                    .add_filter("image", &["png", "gif", "bmp"])
                    .add_filter("PBN", &["xml", "pbn"])
                    .add_filter("chargrid", &["txt"])
                    .add_filter("Olsak", &["g"])
                    .pick_file()
                    .await;

                if let Some(handle) = handle {
                    let document =
                        crate::import::load(&handle.file_name(), handle.read().await, None);

                    sender.send(document).unwrap();
                }
            });
        }

        if let Ok(document) = self.opened_file_receiver.try_recv() {
            self.editor_gui
                .perform(Action::ReplaceDocument { document }, ActionMood::Normal);
        }
    }

    fn saver(&mut self, ui: &mut egui::Ui) {
        if ui.button("Save").clicked() {
            let mut document_copy = self.editor_gui.document.clone();

            spawn_async(async move {
                let handle = rfd::AsyncFileDialog::new()
                    .add_filter(
                        "all recognized formats",
                        &["png", "gif", "bmp", "xml", "pbn", "txt", "g", "html"],
                    )
                    .add_filter("image", &["png", "gif", "bmp"])
                    .add_filter("PBN", &["xml", "pbn"])
                    .add_filter("chargrid", &["txt"])
                    .add_filter("Olsak", &["g"])
                    .add_filter("HTML (for printing)", &["html"])
                    .set_file_name(document_copy.file.clone())
                    .save_file()
                    .await;

                if let Some(handle) = handle {
                    let bytes =
                        to_bytes(&mut document_copy, Some(handle.file_name()), None).unwrap();
                    handle.write(&bytes).await.unwrap();
                }
            });
        }
    }

    fn enter_solve_mode(&mut self) {
        self.solve_mode = true;

        self.solve_gui = Some(crate::gui_solver::SolveGui::new(
            self.editor_gui.document.clone(),
        ));
    }

    pub fn main_ui(&mut self, ctx: &egui::Context, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button(icons::ICON_ZOOM_IN).clicked()
                || ui.input(|i| i.key_pressed(egui::Key::Equals))
            {
                self.scale = (self.scale + 2.0).min(50.0);
            }
            if ui.button(icons::ICON_ZOOM_OUT).clicked()
                || ui.input(|i| i.key_pressed(egui::Key::Minus))
            {
                self.scale = (self.scale - 2.0).max(1.0);
            }
            let picture = self.editor_gui.document.try_solution().unwrap();
            if ui.button("New blank").clicked() {
                self.new_dialog = Some(NewPuzzleDialog {
                    clue_style: picture.clue_style,
                    x_size: picture.x_size(),
                    y_size: picture.y_size(),
                });
            }
            let mut new_document = None;
            if let Some(dialog) = self.new_dialog.as_mut() {
                egui::Window::new("New puzzle").show(ctx, |ui| {
                    ui.add(
                        egui::Slider::new(&mut dialog.x_size, 5..=100)
                            .step_by(5.0)
                            .text("x size"),
                    );
                    ui.add(
                        egui::Slider::new(&mut dialog.y_size, 5..=100)
                            .step_by(5.0)
                            .text("y size"),
                    );
                    ui.radio_value(
                        &mut dialog.clue_style,
                        crate::puzzle::ClueStyle::Nono,
                        "Nonogram",
                    );
                    ui.radio_value(
                        &mut dialog.clue_style,
                        crate::puzzle::ClueStyle::Triano,
                        "Trianogram",
                    );
                    if ui.button("Ok").clicked() {
                        let new_solution = Solution {
                            grid: vec![vec![BACKGROUND; dialog.y_size]; dialog.x_size],
                            palette: match dialog.clue_style {
                                ClueStyle::Nono => import::bw_palette(),
                                ClueStyle::Triano => import::triano_palette(),
                            },
                            clue_style: dialog.clue_style,
                        };
                        new_document = Some(Document::from_solution(
                            new_solution,
                            "blank.xml".to_owned(),
                        ));
                    }
                });
            }

            self.loader(ui);
            if ui.button("Library").clicked() {
                let (sender, receiver) = mpsc::channel();
                self.library_receiver = receiver;

                spawn_async(async move {
                    let result = crate::import::puzzles_from_github().await;
                    if let Ok(library) = result {
                        sender.send(library).unwrap();
                    }
                });
            }

            if let Ok(library) = self.library_receiver.try_recv() {
                self.library_dialog = Some(library);
            }

            let mut close_library = None; // Contains a bool indicating whether to solve
            if let Some(docs) = &self.library_dialog {
                egui::Window::new("Puzzle Library")
                    .max_size(ctx.screen_rect().size() * 0.9)
                    .show(ctx, |ui| {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            egui::Grid::new("library_grid").show(ui, |ui| {
                                for (i, doc) in docs.iter().enumerate() {
                                    if crate::gui_gallery::gallery_puzzle_preview(ui, doc).clicked()
                                    {
                                        new_document = Some(doc.clone());
                                        close_library = Some(true);
                                    }
                                    if i % 2 == 1 {
                                        ui.end_row();
                                    }
                                }
                            });
                        });
                        if ui.button("Cancel").clicked() {
                            close_library = Some(false);
                        }
                    });
            }

            if let Some(new_document) = new_document {
                self.editor_gui.perform(
                    Action::ReplaceDocument {
                        document: new_document,
                    },
                    ActionMood::Normal,
                );
                self.new_dialog = None;
                self.library_dialog = None;
            }
            if let Some(enter_solve_mode) = close_library {
                self.library_dialog = None;
                if enter_solve_mode {
                    self.enter_solve_mode();
                }
            }

            ui.add(
                egui::TextEdit::singleline(&mut self.editor_gui.document.file).desired_width(150.0),
            );
            self.saver(ui);

            ui.separator();
            if ui
                .selectable_value(&mut self.solve_mode, false, "Edit")
                .clicked()
            {
                self.solve_gui = None;
            }
            if ui
                .selectable_value(&mut self.solve_mode, true, "Puzzle")
                .clicked()
            {
                self.enter_solve_mode();
            }
        });
        ui.separator();

        ui.horizontal_top(|ui| {
            if let Some(solve_gui) = &mut self.solve_gui {
                solve_gui.sidebar(ui);
                solve_gui.body(ui, self.scale);
            } else {
                self.edit_sidebar(ui);
                self.editor_gui
                    .canvas(ui, self.scale, RenderStyle::Experimental);
            }
        });
    }
}

struct NewPuzzleDialog {
    clue_style: crate::puzzle::ClueStyle,
    x_size: usize,
    y_size: usize,
}

impl eframe::App for NonogramGui {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Styling. Has to be here instead of `edit_image` to take effect on the Web.
        let spacing = egui::Spacing {
            interact_size: Vec2::new(20.0, 20.0), // Used by the color-picker buttons
            ..egui::Spacing::default()
        };
        let style = Style {
            visuals: Visuals::light(),
            spacing,

            ..Style::default()
        };
        ctx.set_style(style);

        let picture = self.editor_gui.document.solution().unwrap();
        let _background_color = Color32::from_rgb(
            picture.palette[&BACKGROUND].rgb.0,
            picture.palette[&BACKGROUND].rgb.1,
            picture.palette[&BACKGROUND].rgb.2,
        );

        egui::CentralPanel::default().show(ctx, |ui| {
            self.main_ui(ctx, ui);
        });
    }
}

pub struct Disambiguator {
    report: Option<Vec<Vec<(Color, f32)>>>,
    pub terminate_s: mpsc::Sender<()>,
    progress_r: mpsc::Receiver<f32>,
    progress: f32,
    report_r: mpsc::Receiver<Vec<Vec<(Color, f32)>>>,
}

impl Disambiguator {
    pub fn new() -> Self {
        Disambiguator {
            report: None,
            progress: 0.0,
            terminate_s: mpsc::channel().0,
            progress_r: mpsc::channel().1,
            report_r: mpsc::channel().1,
        }
    }

    // Must do this any time the resolution changes!
    // (Currently that only happens through `ReplacePicture`)
    pub fn reset(&mut self) {
        self.report = None;
        self.progress = 0.0;
    }

    pub fn disambig_widget(&mut self, picture: &Solution, ui: &mut egui::Ui) {
        while let Ok(progress) = self.progress_r.try_recv() {
            self.progress = progress;
        }
        let report_running = self.progress > 0.0 && self.progress < 1.0;

        if !report_running {
            if ui.button("Disambiguate!").clicked() {
                let (p_s, p_r) = mpsc::channel();
                let (r_s, r_r) = mpsc::channel();
                let (t_s, t_r) = mpsc::channel();
                self.progress_r = p_r;
                self.terminate_s = t_s;
                self.report_r = r_r;

                let solution = picture.clone();
                spawn_async(async move {
                    let result = disambig_candidates(&solution, p_s, t_r).await;
                    r_s.send(result).unwrap();
                });
            }
        } else {
            if ui.button("Stop").clicked() {
                let _ = self.terminate_s.send(()); // Don't panic if it's already gone!
                self.progress = 0.0;
            }
        }
        if let Ok(report) = self.report_r.try_recv() {
            self.report = Some(report);
        }

        ui.add(egui::ProgressBar::new(self.progress).animate(report_running));
        if ui
            .add_enabled(self.report.is_some(), egui::Button::new("Clear"))
            .clicked()
        {
            self.report = None;
        }
    }
}
