use std::{collections::HashMap, sync::mpsc};

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
enum Tool {
    Pencil,
    FloodFill,
}

use crate::{
    export::to_bytes,
    grid_solve::{self, disambig_candidates},
    gui_solver::{SolveGui, draw_dyn_row_clues},
    import,
    puzzle::{BACKGROUND, ClueStyle, Color, ColorInfo, Corner, Document, Solution, UNSOLVED},
};
use egui::{Color32, Pos2, Rect, RichText, Shape, Style, Vec2, Visuals};
use egui_material_icons::icons;

#[cfg(not(target_arch = "wasm32"))]
pub fn edit_image(solution: Solution) {
    let native_options = eframe::NativeOptions::default();
    eframe::run_native(
        "Number Loom",
        native_options,
        Box::new(|cc| Ok(Box::new(NonogramGui::new(cc, solution)))),
    )
    .unwrap()
}

#[cfg(target_arch = "wasm32")]
pub fn edit_image(solution: Solution) {
    use eframe::wasm_bindgen::JsCast as _;

    let web_options = eframe::WebOptions::default();

    wasm_bindgen_futures::spawn_local(async {
        let document = web_sys::window()
            .expect("No window")
            .document()
            .expect("No document");

        let canvas = document
            .get_element_by_id("the_canvas_id")
            .expect("Failed to find the_canvas_id")
            .dyn_into::<web_sys::HtmlCanvasElement>()
            .expect("the_canvas_id was not a HtmlCanvasElement");

        let start_result = eframe::WebRunner::new()
            .start(
                canvas,
                web_options,
                Box::new(|cc| Ok(Box::new(NonogramGui::new(cc, solution)))),
            )
            .await;

        // Remove the loading text and spinner:
        if let Some(loading_text) = document.get_element_by_id("loading_text") {
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

struct NonogramGui {
    picture: Solution,
    file_name: String,
    current_color: Color,
    drag_start_color: Color,
    scale: f32,
    opened_file_receiver: mpsc::Receiver<(Solution, String)>,
    new_dialog: Option<NewPuzzleDialog>,
    auto_solve: bool,
    lines_to_affect_string: String,

    undo_stack: Vec<Action>,
    redo_stack: Vec<Action>,

    solve_report: String,
    report_stale: bool,
    disambiguator: Disambiguator,
    solved_mask: Vec<Vec<bool>>,
    current_tool: Tool,

    solve_mode: bool,
    solve_gui: Option<SolveGui>,
}

#[derive(Clone, Debug)]
enum Action {
    ChangeColor {
        changes: HashMap<(usize, usize), Color>,
    },
    ReplacePicture {
        picture: Solution,
    },
}

#[derive(PartialEq, Eq)]
enum ActionMood {
    Normal,
    Merge,
    Undo,
    Redo,
}

impl NonogramGui {
    fn new(cc: &eframe::CreationContext<'_>, picture: Solution) -> Self {
        egui_material_icons::initialize(&cc.egui_ctx);
        let solved_mask = vec![vec![false; picture.grid[0].len()]; picture.grid.len()];

        let mut current_color = BACKGROUND;
        for (c, ci) in picture.palette.iter() {
            if ci.rgb == (0, 0, 0) && ci.corner.is_none() {
                current_color = *c;
            }
        }

        NonogramGui {
            picture,
            file_name: "blank.xml".to_string(),
            current_color,
            drag_start_color: current_color,
            scale: 16.0,
            opened_file_receiver: mpsc::channel().1,
            new_dialog: None,
            auto_solve: false,
            lines_to_affect_string: "5".to_string(),

            undo_stack: vec![],
            redo_stack: vec![],

            solve_report: "".to_string(),
            report_stale: true,
            disambiguator: Disambiguator::new(),
            solved_mask,
            current_tool: Tool::Pencil,

            solve_mode: false,
            solve_gui: None,
        }
    }

    fn reversed(&self, action: &Action) -> Action {
        match action {
            Action::ChangeColor { changes } => Action::ChangeColor {
                changes: changes
                    .keys()
                    .map(|(x, y)| ((*x, *y), self.picture.grid[*x][*y]))
                    .collect::<HashMap<_, _>>(),
            },
            Action::ReplacePicture { picture: _ } => Action::ReplacePicture {
                picture: self.picture.clone(),
            },
        }
    }

    fn perform(&mut self, action: Action, mood: ActionMood) {
        use Action::*;
        use ActionMood::*;

        let mood = if mood == Merge {
            match (self.undo_stack.last_mut(), &action) {
                // Consecutive `ChangeColor`s can be merged with each other.
                (
                    Some(ChangeColor { changes }),
                    ChangeColor {
                        changes: new_changes,
                    },
                ) => {
                    for ((x, y), col) in new_changes {
                        if !changes.contains_key(&(*x, *y)) {
                            changes.insert((*x, *y), self.picture.grid[*x][*y]);
                            // Crucially, this only fires on a new cell!
                            // Otherwise, we'd be flipping cells back and forth as long as we were
                            // in them!
                            self.picture.grid[*x][*y] = *col;
                            self.report_stale = true;
                        }
                    }
                    return; // Action is done; nothing else to do!
                }
                _ => Normal, // Unable to merge; add a new undo entry.
            }
        } else {
            mood
        };

        let reversed_action = self.reversed(&action);

        match action {
            Action::ChangeColor { changes } => {
                for ((x, y), new_color) in changes {
                    self.picture.grid[x][y] = new_color;
                }
                self.report_stale = true;
            }
            Action::ReplacePicture { picture } => {
                let solved_mask = vec![vec![false; picture.grid[0].len()]; picture.grid.len()];
                self.picture = picture;
                self.solved_mask = solved_mask;

                self.report_stale = true;
                self.disambiguator.reset();
            }
        }

        match mood {
            Merge => {}
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

    fn un_or_re_do(&mut self, un: bool) {
        let action = if un {
            self.undo_stack.pop()
        } else {
            self.redo_stack.pop()
        };

        let action = match action {
            Some(action) => action,
            None => return,
        };

        self.perform(
            action,
            if un {
                ActionMood::Undo
            } else {
                ActionMood::Redo
            },
        );
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
) -> Vec<egui::Shape> {
    let (r, g, b) = ci.rgb;
    let color = egui::Color32::from_rgb(r, g, b);

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

    if !solved {
        res.push(egui::Shape::circle_filled(
            to_screen * Pos2::new(x as f32 + 0.5, y as f32 + 0.5),
            to_screen.scale().x * 0.3,
            egui::Color32::from_rgb(128, 128, 128),
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
    fn resize(&mut self, top: Option<bool>, left: Option<bool>, add: bool) {
        let mut g = self.picture.grid.clone();
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

        self.perform(
            Action::ReplacePicture {
                picture: Solution {
                    grid: g,
                    ..self.picture.clone()
                },
            },
            ActionMood::Normal,
        );
    }

    fn resizer(&mut self, ui: &mut egui::Ui) {
        ui.label(format!(
            "Canvas size: {}x{}",
            self.picture.x_size(),
            self.picture.y_size(),
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

    fn flood_fill(&mut self, x: usize, y: usize) {
        let target_color = self.picture.grid[x][y];
        if target_color == self.current_color {
            return; // Nothing to do
        }

        let mut changes = HashMap::new();
        let mut q = std::collections::VecDeque::new();

        q.push_back((x, y));
        let mut visited = std::collections::HashSet::new();
        visited.insert((x, y));

        let x_size = self.picture.grid.len();
        let y_size = self.picture.grid.first().unwrap().len();

        while let Some((px, py)) = q.pop_front() {
            changes.insert((px, py), self.current_color);

            let neighbors = [
                (px.wrapping_sub(1), py),
                (px + 1, py),
                (px, py.wrapping_sub(1)),
                (px, py + 1),
            ];

            for (nx, ny) in neighbors {
                if nx < x_size && ny < y_size && self.picture.grid[nx][ny] == target_color {
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
                Tool::FloodFill,
                egui::RichText::new(icons::ICON_FORMAT_COLOR_FILL).size(24.0),
            )
            .on_hover_text("Flood Fill");
        });
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.vertical(|ui| {
            ui.set_width(120.0);
            ui.horizontal(|ui| {
                ui.label(format!("({})", self.undo_stack.len()));
                if ui.button(icons::ICON_UNDO).clicked()
                    || ui.input(|i| i.key_pressed(egui::Key::Z))
                {
                    self.un_or_re_do(true);
                }
                if ui.button(icons::ICON_REDO).clicked()
                    || ui.input(|i| i.key_pressed(egui::Key::Y))
                {
                    self.un_or_re_do(false);
                }
                ui.label(format!("({})", self.redo_stack.len()));
            });

            ui.separator();

            self.tool_selector(ui);

            ui.separator();

            self.resizer(ui);

            ui.separator();

            self.palette_editor(ui);

            ui.separator();
            ui.checkbox(&mut self.auto_solve, "auto-solve");
            if ui.button("Solve").clicked() || (self.auto_solve && self.report_stale) {
                let puzzle = self.picture.to_puzzle();

                match puzzle.plain_solve() {
                    Ok(grid_solve::Report {
                        solve_counts,
                        cells_left,
                        solution: _solution,
                        solved_mask,
                    }) => {
                        self.solve_report = format!("{solve_counts} unsolved cells: {cells_left}");
                        self.solved_mask = solved_mask;
                    }
                    Err(e) => self.solve_report = format!("Error: {:?}", e),
                }
                self.report_stale = false;
            }

            ui.colored_label(
                if self.report_stale {
                    Color32::GRAY
                } else {
                    Color32::BLACK
                },
                &self.solve_report,
            );

            ui.separator();

            Disambiguator::disambig_widget(&mut self.disambiguator, &self.picture, ui);

            if self.disambiguator.report.is_some()
                || (self.disambiguator.progress > 0.0 && self.disambiguator.progress < 1.0)
            {
                self.report_stale = true; // hide the dots while disambiguating
            }
        });
    }

    fn palette_editor(&mut self, ui: &mut egui::Ui) {
        let mut picked_color = self.current_color;
        let mut removed_color = None;
        let mut add_color = false;

        use itertools::Itertools;

        for (color, color_info) in self
            .picture
            .palette
            .iter_mut()
            .sorted_by_key(|(color, _)| *color)
        {
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
            });
        }
        if ui.button("New color").clicked() {
            add_color = true;
        }
        self.current_color = picked_color;

        if Some(self.current_color) == removed_color {
            self.current_color = BACKGROUND;
        }

        if let Some(removed_color) = removed_color {
            let mut new_picture = self.picture.clone();
            for row in new_picture.grid.iter_mut() {
                for cell in row.iter_mut() {
                    if *cell == removed_color {
                        *cell = self.current_color;
                    }
                }
            }
            new_picture.palette.remove(&removed_color);
            self.perform(
                Action::ReplacePicture {
                    picture: new_picture,
                },
                ActionMood::Normal,
            );
        }
        if add_color {
            let next_color = Color(self.picture.palette.keys().map(|k| k.0).max().unwrap() + 1);
            let mut new_picture = self.picture.clone();
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
                Action::ReplacePicture {
                    picture: new_picture,
                },
                ActionMood::Normal,
            );
        }
    }

    fn canvas(&mut self, ui: &mut egui::Ui) {
        let x_size = self.picture.grid.len();
        let y_size = self.picture.grid.first().unwrap().len();

        let (mut response, painter) = ui.allocate_painter(
            Vec2::new(self.scale * x_size as f32, self.scale * y_size as f32) + Vec2::new(2.0, 2.0), // for the border
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
                match self.current_tool {
                    Tool::Pencil => {
                        if response.clicked() || response.dragged() {
                            let new_color = if self.picture.grid[x][y] == self.current_color {
                                BACKGROUND
                            } else {
                                self.current_color
                            };
                            let mood = if response.clicked() || response.drag_started() {
                                self.drag_start_color = new_color;
                                ActionMood::Normal
                            } else {
                                ActionMood::Merge
                            };

                            let mut changes = HashMap::new();
                            changes.insert((x, y), self.drag_start_color);
                            self.perform(Action::ChangeColor { changes }, mood);
                        }
                    }
                    Tool::FloodFill => {
                        if response.clicked() {
                            self.flood_fill(x, y);
                        }
                    }
                }
            }
        }

        let mut shapes = vec![];
        let disambig_report = &self.disambiguator.report;

        for y in 0..y_size {
            for x in 0..x_size {
                let cell = self.picture.grid[x][y];
                let color_info = &self.picture.palette[&cell];
                let solved = self.solved_mask[x][y] || self.report_stale;

                let dr = if let Some(disambig_report) = disambig_report.as_ref() {
                    let (c, score) = disambig_report[x][y];
                    (&self.picture.palette[&c], score)
                } else {
                    (&self.picture.palette[&BACKGROUND], 1.0)
                };

                for shape in cell_shape(color_info, solved, dr, x, y, &to_screen) {
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

                    sender
                        .send((document.take_solution().unwrap(), handle.file_name()))
                        .unwrap();
                }
            });
        }

        if let Ok((solution, file)) = self.opened_file_receiver.try_recv() {
            self.perform(
                Action::ReplacePicture { picture: solution },
                ActionMood::Normal,
            );
            self.file_name = file;
        }
    }

    fn saver(&mut self, ui: &mut egui::Ui) {
        if ui.button("Save").clicked() {
            let solution_copy = self.picture.clone();
            let file_copy = self.file_name.clone();

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
                    .set_file_name(file_copy)
                    .save_file()
                    .await;

                if let Some(handle) = handle {
                    let mut document = Document::new(None, Some(solution_copy), handle.file_name());
                    let bytes = to_bytes(&mut document, Some(handle.file_name()), None).unwrap();
                    handle.write(&bytes).await.unwrap();
                }
            });
        }
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

        let _background_color = Color32::from_rgb(
            self.picture.palette[&BACKGROUND].rgb.0,
            self.picture.palette[&BACKGROUND].rgb.1,
            self.picture.palette[&BACKGROUND].rgb.2,
        );

        egui::CentralPanel::default().show(ctx, |ui| {
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
                if ui.button("New blank").clicked() {
                    self.new_dialog = Some(NewPuzzleDialog {
                        clue_style: self.picture.clue_style,
                        x_size: self.picture.x_size(),
                        y_size: self.picture.y_size(),
                    });
                }
                let mut new_picture = None;
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
                            new_picture = Some(Solution {
                                grid: vec![vec![BACKGROUND; dialog.y_size]; dialog.x_size],
                                palette: match dialog.clue_style {
                                    ClueStyle::Nono => import::bw_palette(),
                                    ClueStyle::Triano => import::triano_palette(),
                                },
                                clue_style: dialog.clue_style,
                            });
                        }
                    });
                }

                if let Some(new_picture) = new_picture {
                    self.perform(
                        Action::ReplacePicture {
                            picture: new_picture,
                        },
                        ActionMood::Normal,
                    );
                    self.new_dialog = None;
                }

                self.loader(ui);
                ui.add(egui::TextEdit::singleline(&mut self.file_name).desired_width(150.0));
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
                    let mut blank_solution = self.picture.clone();
                    for row in blank_solution.grid.iter_mut() {
                        for cell in row.iter_mut() {
                            *cell = UNSOLVED;
                        }
                    }

                    self.solve_gui = Some(crate::gui_solver::SolveGui {
                        partial_solution: blank_solution,
                        clues: self.picture.to_puzzle(),
                        current_color: self.current_color,
                    });
                }
            });
            ui.separator();

            ui.horizontal(|ui| {
                match &self.solve_gui {
                    None => {
                        self.sidebar(ui);
                    }
                    Some(solve_gui) => {
                        draw_dyn_row_clues(ui, &solve_gui.clues, self.scale);
                    }
                }

                self.canvas(ui);
            });
        });
    }
}

struct Disambiguator {
    report: Option<Vec<Vec<(Color, f32)>>>,
    terminate_s: mpsc::Sender<()>,
    progress_r: mpsc::Receiver<f32>,
    progress: f32,
    report_r: mpsc::Receiver<Vec<Vec<(Color, f32)>>>,
}

impl Disambiguator {
    fn new() -> Self {
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
    fn reset(&mut self) {
        self.report = None;
        self.progress = 0.0;
    }

    fn disambig_widget(&mut self, picture: &Solution, ui: &mut egui::Ui) {
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
