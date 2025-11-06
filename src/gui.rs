use std::sync::mpsc;

use crate::{
    export::to_bytes,
    grid_solve::{self, disambig_candidates},
    gui_canvas::{Action, ActionMood, CanvasGui, Staleable, Tool},
    gui_solver::{RenderStyle, SolveGui},
    import,
    puzzle::{BACKGROUND, ClueStyle, Color, Document, PuzzleDynOps, Solution},
    user_settings::{consts, UserSettings},
};
use egui::{Color32, Style, TextStyle, Vec2, Visuals};
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
    show_share_window: bool,
    share_string: String,
    pasted_string: String,
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
                id: Staleable {
                    val: "".to_string(),
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
            show_share_window: false,
            share_string: "".to_string(),
            pasted_string: "".to_string(),
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
            let backup_title = self.editor_gui.document.get_or_make_up_title().unwrap();
            let id = self
                .editor_gui
                .id
                .get_or_refresh(self.editor_gui.version, || backup_title.clone());
            if self.editor_gui.document.id != *id {
                self.editor_gui.document.id = id.clone();
            }

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

            let cc_by_license_str = "CC BY 4.0";
            let mut is_cc_by = self.editor_gui.document.license == cc_by_license_str;

            ui.separator();
            ui.label("License:");

            ui.horizontal(|ui| {
                if ui.radio_value(&mut is_cc_by, true, "").changed() {
                    self.editor_gui.document.license = cc_by_license_str.to_string();
                };
                ui.add(
                    egui::Hyperlink::from_label_and_url(
                        cc_by_license_str,
                        "https://creativecommons.org/licenses/by/4.0/",
                    )
                    .open_in_new_tab(true),
                );
            });

            ui.horizontal(|ui| {
                if ui.radio_value(&mut is_cc_by, false, "").changed() {
                    self.editor_gui.document.license.clear();
                };
                ui.add_enabled(
                    !is_cc_by,
                    egui::TextEdit::singleline(&mut self.editor_gui.document.license),
                );
            });
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
            let picture = self.editor_gui.document.solution_mut();
            if ui.button("New").clicked() {
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
                        self.solve_mode = false;
                    }
                });
            }

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

            let mut next_enter_solve_mode = false;
            let mut close_library = false;
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
                                        next_enter_solve_mode = true;
                                        close_library = true;
                                    }
                                    if i % 2 == 1 {
                                        ui.end_row();
                                    }
                                }
                            });
                        });
                        if ui.button("Cancel").clicked() {
                            close_library = true;
                        }
                    });
            }
            if close_library {
                self.library_dialog = None;
            }
            self.loader(ui);

            ui.add(
                egui::TextEdit::singleline(&mut self.editor_gui.document.file).desired_width(150.0),
            );
            self.saver(ui);

            if ui.button("Share").clicked() {
                self.share_string =
                    crate::formats::woven::to_share_string(&mut self.editor_gui.document).unwrap();
                self.show_share_window = true;
            }

            if self.show_share_window {
                egui::Window::new("Share Puzzle")
                    .open(&mut self.show_share_window)
                    .default_width(780.0)
                    .show(ctx, |ui| {
                        ui.label("Share String:");
                        ui.add(
                            egui::TextEdit::multiline(&mut self.share_string.clone())
                                .font(TextStyle::Monospace)
                                .desired_width(730.0),
                        );
                        if ui.button("Copy to clipboard").clicked() {
                            ctx.copy_text(self.share_string.clone());
                        }

                        ui.separator();

                        ui.label("Paste a share string to load:");
                        ui.add(
                            egui::TextEdit::multiline(&mut self.pasted_string)
                                .font(TextStyle::Monospace)
                                .desired_width(730.0),
                        );

                        if ui.button("Load").clicked() {
                            match crate::formats::woven::from_share_string(&self.pasted_string) {
                                Ok(doc) => {
                                    new_document = Some(doc);
                                    next_enter_solve_mode = true;
                                }
                                Err(e) => {
                                    // TODO: we probably need to make statuses coherent somehow
                                    self.solve_report = format!("Error: {:?}", e);
                                }
                            }
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
                self.show_share_window = false;
            }

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
                || next_enter_solve_mode
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

        egui::CentralPanel::default().show(ctx, |ui| {
            self.main_ui(ctx, ui);
        });
    }
}

pub struct Disambiguator {
    pub report: Option<Vec<Vec<(Color, f32)>>>,
    pub terminate_s: mpsc::Sender<()>,
    pub progress_r: mpsc::Receiver<f32>,
    pub progress: f32,
    pub report_r: mpsc::Receiver<Vec<Vec<(Color, f32)>>>,
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
