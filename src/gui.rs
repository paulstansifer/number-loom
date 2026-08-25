mod annotate;
mod canvas;
pub mod gallery;
mod palette;
mod resize;
mod selection;
pub mod solver;
mod toolbar;
mod tools;

pub use annotate::{AnnotateDrag, Annotation};
pub use canvas::{ClueOverlay, HoverBlocks, triangle_shape};
pub use palette::default_color;
pub use selection::{Selection, cells_in_lasso};
pub use toolbar::LibraryStatus;
use toolbar::NewPuzzleDialog;
pub use tools::Tool;

use std::{cell::RefCell, collections::HashMap, rc::Rc, sync::mpsc, time::Duration};

use web_time::Instant;

#[derive(Clone)]
pub struct StatusMessage {
    pub text: String,
    pub is_error: bool,
}

impl StatusMessage {
    pub fn info(text: impl Into<String>) -> Self {
        StatusMessage {
            text: text.into(),
            is_error: false,
        }
    }

    pub fn error(text: impl Into<String>) -> Self {
        StatusMessage {
            text: text.into(),
            is_error: true,
        }
    }
}

const STATUS_GRACE_PERIOD: Duration = Duration::from_secs(1);

// Shared between `CanvasGui`s so that the editor and the solver (which have separate
// `CanvasGui`s) can show messages in the same status bar.
pub struct StatusCell {
    // The `Instant` is when the message was set: for the first `STATUS_GRACE_PERIOD` after that,
    // `maybe_clear_on_dirty` calls are ignored, so a message doesn't disappear before the user
    // has had a chance to read it.
    inner: RefCell<Option<(StatusMessage, Instant)>>,
}

impl StatusCell {
    pub fn new() -> Rc<Self> {
        Rc::new(StatusCell {
            inner: RefCell::new(None),
        })
    }

    pub fn set(&self, message: StatusMessage) {
        *self.inner.borrow_mut() = Some((message, Instant::now()));
    }

    pub fn get(&self) -> Option<StatusMessage> {
        self.inner
            .borrow()
            .as_ref()
            .map(|(message, _)| message.clone())
    }

    // Called when something dirties the editor (or otherwise makes the current message stale);
    // clears the message, unless it was set too recently for the user to have read it yet.
    pub fn maybe_clear_on_dirty(&self) {
        let mut inner = self.inner.borrow_mut();
        if let Some((_, set_at)) = *inner
            && set_at.elapsed() >= STATUS_GRACE_PERIOD
        {
            *inner = None;
        }
    }
}

pub type SharedStatus = Rc<StatusCell>;

// Shared the same way as `SharedStatus`, for a progress bar in the status bar. `Some(fraction)`
// (0.0 to 1.0) while a long-running task is in progress, `None` otherwise.
pub type SharedProgress = Rc<RefCell<Option<f32>>>;

use solver::{RenderStyle, SolveGui};

use crate::{
    export::to_bytes,
    grid_solve::{self, DisambigResult, disambig_candidates},
    import,
    // The abstract-units point, distinct from egui's `Pos2`: everything the lasso does is in
    // grid space, and only the painter converts.
    layout::Point,
    puzzle::{
        BACKGROUND, Clue, ClueStyle, Color, ColorInfo, Corner, Document, DynSolution, Palette,
        PuzzleDynOps, Solution, UNSOLVED,
    },
    user_settings::{UserSettings, consts},
};
use egui::{Color32, Pos2, Rect, RichText, Shape, Style, TextStyle, Vec2, Visuals};

/// The editor still only understands rows and columns. Rather than panicking deep inside a
/// drawing routine, every entry point that needs a square picture says so here.
const TRIDDLER_UNSUPPORTED: &str = "the editor can't edit triddlers yet";
use egui_material_icons::icons;

#[cfg(not(target_arch = "wasm32"))]
pub fn edit_image(document: Document) {
    use eframe::icon_data::from_png_bytes;
    use egui::ViewportBuilder;

    let icon_bytes: &'static [u8] = include_bytes!("../icon.png");

    let native_options = eframe::NativeOptions {
        viewport: ViewportBuilder::default()
            .with_inner_size(Vec2::new(800.0, 800.0))
            .with_app_id("Number Loom")
            .with_icon(from_png_bytes(icon_bytes).unwrap()),
        persist_window: true,
        ..eframe::NativeOptions::default()
    };

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

    pub fn get_or_refresh<F>(&mut self, version: Version, refresh: F) -> &mut T
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
    pub line_tool_state: Option<u32>,
    /// The lasso tool's selection, if any. Outlives switching tools only long enough to be
    /// flattened; see `flatten_selection`.
    pub selection: Option<Selection>,
    /// The annotate tool's scratch marks. Deliberately outside the undo system: none of these is
    /// an `Action`, and none of them bumps `version`.
    pub annotations: Vec<Annotation>,
    /// The annotation being dragged out right now, if any.
    pub annotate_drag: Option<AnnotateDrag>,
    /// Whether this canvas offers the annotate tool at all — true only in the solver, the way
    /// flood fill and the lasso are editor-only. Gates the button, the `A` key and the shift
    /// momentary alike.
    pub allow_annotations: bool,
    /// Indexed by dense cell index, like `Solution::cells`.
    pub solved_mask: Staleable<(String, Vec<bool>)>,
    pub disambiguator: Staleable<Disambiguator>,
    pub id: Staleable<String>,
    pub status: SharedStatus,
    pub progress: SharedProgress,
    /// Where the picture itself (not counting any clue gutters) landed on screen, as of the last
    /// frame that drew it. `pub` for tests/gui.rs, which needs a point that's reliably on the
    /// canvas to aim a click at — a hardcoded one goes stale the moment the layout around it
    /// moves.
    pub picture_rect: Option<Rect>,
}

pub struct NonogramGui {
    // The `pub`s are solely for tests/gui.rs
    pub editor_gui: CanvasGui,
    scale: f32,
    opened_file_receiver: mpsc::Receiver<anyhow::Result<Document>>,
    save_result_receiver: mpsc::Receiver<anyhow::Result<()>>,
    library_receiver: mpsc::Receiver<anyhow::Result<Vec<Document>>>,
    library_dialog: Option<LibraryStatus>,
    new_dialog: Option<NewPuzzleDialog>,
    auto_solve: bool,
    lines_to_affect_string: String,
    solve_report: String,
    pub solve_mode: bool,
    pub solve_gui: Option<SolveGui>,
    show_save_share_window: bool,
    share_string: String,
    pasted_string: String,
    quality_warnings: Vec<String>,
}

#[derive(Clone, Debug)]
pub enum Action {
    /// Keyed by dense cell index rather than by coordinate: every consumer wants a cell, and
    /// indices make undo, the tools, and merging work for any shape with no dispatch at all.
    /// Coordinates appear only at the hit-test boundary, as `DynCoord`.
    ChangeColor {
        changes: HashMap<u32, Color>,
    },
    ReplaceDocument {
        document: Box<Document>,
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
    /// Sync `self.current_color` and `self.drag_start_color` to the document, for safety.
    fn clamp_colors_to_palette(&mut self) {
        let Some(picture) = self.document.try_solution() else {
            return;
        };
        let palette = picture.palette();
        let fallback = default_color(palette);

        if !palette.contains_key(&self.current_color) {
            self.current_color = fallback;
        }
        if !palette.contains_key(&self.drag_start_color) {
            self.drag_start_color = fallback;
        }
    }

    fn reversed(&self, action: &Action) -> Action {
        match action {
            Action::ChangeColor { changes } => {
                let cells = self.document.try_solution().unwrap().cells();
                Action::ChangeColor {
                    changes: changes
                        .keys()
                        .map(|cell| (*cell, cells[*cell as usize]))
                        .collect(),
                }
            }
            Action::ReplaceDocument { document: _ } => Action::ReplaceDocument {
                document: Box::new(self.document.clone()),
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
                    let cells = self.document.solution_mut().cells_mut();
                    if mood == ReplaceAction {
                        for cell in new_changes.keys() {
                            changes.entry(*cell).or_insert(cells[*cell as usize]);
                        }
                        changes.retain(|cell, old_col| {
                            if !new_changes.contains_key(cell) {
                                cells[*cell as usize] = *old_col;
                                self.version += 1;
                                false
                            } else {
                                true
                            }
                        });
                        for (cell, col) in new_changes {
                            if cells[*cell as usize] != *col {
                                cells[*cell as usize] = *col;
                                self.version += 1;
                            }
                        }
                        return;
                    } else {
                        for (cell, col) in new_changes {
                            if !changes.contains_key(cell) {
                                changes.insert(*cell, cells[*cell as usize]);
                                // Crucially, this only fires on a new cell!
                                // Otherwise, we'd be flipping cells back and forth as long as we
                                // were in them!
                                cells[*cell as usize] = *col;
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

        let version_before = self.version;
        match action {
            Action::ChangeColor { changes } => {
                let cells = self.document.solution_mut().cells_mut();
                for (cell, new_color) in changes {
                    if cells[cell as usize] != new_color {
                        cells[cell as usize] = new_color;
                        self.version += 1;
                    }
                }
            }
            Action::ReplaceDocument { document } => {
                let mut document = document;
                if let Ok(true) = document.has_complete_solution() {
                    self.document = *document;
                    self.version += 1;
                    // A mask means nothing against a picture that was swapped out from under it,
                    // and a floating layer belongs to the picture it was lifted from. Annotations
                    // name lanes, which the new picture may not have at all.
                    self.selection = None;
                    self.annotations.clear();
                    self.annotate_drag = None;
                    // The new palette may not have the color the old one did.
                    self.clamp_colors_to_palette();
                } else {
                    self.status
                        .set(StatusMessage::error("That puzzle has no solution"));
                }
            }
        }
        if self.version != version_before {
            self.status.maybe_clear_on_dirty();
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

    /// `editing` is false in the solver, which shares this sidebar but must not offer the tools
    /// that rearrange the picture.
    pub fn common_sidebar_items(
        &mut self,
        ui: &mut egui::Ui,
        palette_read_only: bool,
        editing: bool,
    ) {
        // A focused `TextEdit` reads its key events without consuming them, so a bare-key
        // shortcut still fires while the user is typing into one. Nothing here uses a modifier,
        // so every one of them has to be suppressed by hand.
        let typing = ui.ctx().wants_keyboard_input();

        let (can_undo, can_redo) = (!self.undo_stack.is_empty(), !self.redo_stack.is_empty());

        centered_row(ui, "undo_row", |ui| {
            ui.label(format!("({})", self.undo_stack.len()));
            if ui
                .add_enabled(can_undo, egui::Button::new(icons::ICON_UNDO))
                .clicked()
                || (can_undo && !typing && ui.input(|i| i.key_pressed(egui::Key::Z)))
            {
                self.un_or_re_do(true);
            }
            if ui
                .add_enabled(can_redo, egui::Button::new(icons::ICON_REDO))
                .clicked()
                || (can_redo && !typing && ui.input(|i| i.key_pressed(egui::Key::Y)))
            {
                self.un_or_re_do(false);
            }
            ui.label(format!("({})", self.redo_stack.len()));
        });

        ui.separator();

        self.tool_selector(ui, editing);

        // Annotations aren't part of the picture, so undo can't take them back — this is the only
        // way to be rid of them, and it only appears when there's something to clear.
        if self.allow_annotations && !self.annotations.is_empty() {
            centered_row(ui, "clear_annotations", |ui| {
                if ui
                    .button("Clear annotations")
                    .on_hover_text("Remove every annotation (press Escape)")
                    .clicked()
                    || (!typing && ui.input(|i| i.key_pressed(egui::Key::Escape)))
                {
                    self.annotations.clear();
                    self.annotate_drag = None;
                }
            });
        }

        ui.separator();

        self.palette_editor(ui, palette_read_only);
    }
}

impl NonogramGui {
    pub fn new(mut document: Document) -> Self {
        // (Public for testing)
        //
        // A document loaded from a clue-only format (olsak, webpbn) has no picture yet, so solve
        // for one. A self-contradictory puzzle can't produce one at all; rather than panicking,
        // fall back to a blank canvas and say so once the status cell exists.
        let mut load_error = None;
        if let Err(e) = document.solution() {
            load_error = Some(format!("could not solve this puzzle: {e}"));
            document = Document::from_solution(
                DynSolution::Square(Solution::blank_bw(20, 20)),
                document.file.clone(),
            );
        }

        let picture = document.try_solution().expect("just ensured there is one");
        let solved_mask = vec![true; picture.cells().len()];

        let current_color = default_color(picture.palette());

        if document.author.is_empty()
            && let Some(author) = UserSettings::get(consts::EDITOR_AUTHOR_NAME)
        {
            document.author = author;
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
                selection: None,
                annotations: vec![],
                annotate_drag: None,
                allow_annotations: false,
                picture_rect: None,
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
                status: {
                    let status = StatusCell::new();
                    if let Some(message) = load_error {
                        status.set(StatusMessage::error(message));
                    }
                    status
                },
                progress: Rc::new(RefCell::new(None)),
            },
            scale: 16.0,
            opened_file_receiver: mpsc::channel().1,
            save_result_receiver: mpsc::channel().1,
            library_receiver: mpsc::channel().1,
            new_dialog: None,
            library_dialog: None,
            auto_solve: UserSettings::get_bool(consts::EDITOR_AUTO_SOLVE),
            lines_to_affect_string: "5".to_string(),
            solve_report: "".to_string(),
            solve_mode: false,
            solve_gui: None,
            show_save_share_window: false,
            share_string: "".to_string(),
            pasted_string: "".to_string(),
            quality_warnings: vec![],
        }
    }

    fn edit_sidebar(&mut self, ui: &mut egui::Ui) {
        ui.vertical(|ui| {
            // The id tracks the title, and `Save/share` needs it whether or not the "Metadata"
            // section happens to be expanded — so this can't live inside that section's body,
            // which egui skips entirely while it's collapsed.
            let backup_title = self.editor_gui.document.get_or_make_up_title().unwrap();
            let id = self
                .editor_gui
                .id
                .get_or_refresh(self.editor_gui.version, || backup_title.clone());
            if self.editor_gui.document.id != *id {
                self.editor_gui.document.id = id.clone();
            }

            self.metadata_editor(ui);

            ui.separator();

            self.editor_gui.common_sidebar_items(ui, false, true);

            ui.separator();

            match self.editor_gui.document.try_solution().map(|s| s.shape()) {
                Some(crate::geometry::Shape::Triangular(_)) => self.tri_resizer(ui),
                _ => self.resizer(ui),
            }

            ui.separator();
            if ui.checkbox(&mut self.auto_solve, "auto-solve").changed() {
                let _ = UserSettings::set(consts::EDITOR_AUTO_SOLVE, &self.auto_solve.to_string());
                if !self.auto_solve {
                    // The shading clears itself (it's only drawn while fresh), but the report is
                    // plain text that would otherwise linger after the aid is switched off.
                    self.solve_report.clear();
                }
            }
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
                                // Unsolved cells first: that's the number that says whether the
                                // puzzle works. The skim/scrub counts are solver diagnostics.
                                format!("unsolved cells: {cells_left}\n{solve_counts}"),
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

            let picture = self.editor_gui.document.try_solution().unwrap().clone();
            self.editor_gui
                .disambiguator
                .get_or_refresh(self.editor_gui.version, Disambiguator::new)
                .disambig_widget(
                    &picture,
                    &self.editor_gui.status,
                    &self.editor_gui.progress,
                    ui,
                );
        });
    }

    /// Title, author, description and license; in a collapsable section.
    fn metadata_editor(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Metadata").show(ui, |ui| {
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

            ui.label("Description:");
            ui.text_edit_multiline(&mut self.editor_gui.document.description);

            let cc_by_license_str = "CC BY 4.0";
            let mut is_cc_by = self.editor_gui.document.license == cc_by_license_str;

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
    fn enter_solve_mode(&mut self) {
        self.solve_mode = true;

        self.solve_gui = Some(SolveGui::new(
            self.editor_gui.document.clone(),
            Rc::clone(&self.editor_gui.status),
            Rc::clone(&self.editor_gui.progress),
        ));
    }
}

/// Starting width of the tool sidebar; the user may adjust it.
const SIDEBAR_WIDTH: f32 = 150.0;

/// Lay out a row of widgets centred within the available width.
///
/// egui won't do this on its own. `Layout::left_to_right` places items from the left edge
/// whatever its `main_align` says — `horizontal_placement` hardcodes `Align::LEFT` — and
/// `vertical_centered` only centres a child whose size it knows *before* laying it out, which a
/// `ui.horizontal` row is not. The obvious fix, a sizing pass followed by a real one, would mean
/// running `add_contents` twice per frame; these rows are made of buttons, so the measuring pass
/// would act on every click a second time.
///
/// So: pad by however wide the row turned out last frame, and record this frame's width for the
/// next one. Only the very first frame a row is shown is off-centre.
fn centered_row<R>(
    ui: &mut egui::Ui,
    id_salt: &str,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let id = ui.id().with(id_salt);
    let previous: Option<f32> = ui.data(|d| d.get_temp(id));
    let pad = previous.map_or(0.0, |w| ((ui.available_width() - w) / 2.0).max(0.0));

    let row = ui.horizontal(|ui| {
        ui.add_space(pad);
        add_contents(ui)
    });

    ui.data_mut(|d| d.insert_temp(id, row.response.rect.width() - pad));
    row.inner
}

/// Which edge of a `TopBottomPanel` faces the rest of the window, and so carries its separator.
enum Edge {
    Top,
    Bottom,
}

/// The frame for the toolbar and the status bar.
///
/// `TopBottomPanel` paints its separator line *inside* its own rect, overlapping the frame's
/// margin on whichever edge faces the window. With egui's symmetric default margin that leaves
/// only one pixel of clear space between the line and the buttons on that side, against two on
/// the other — which reads as the bar being a pixel or two too short for its contents. Widening
/// that one margin by the line's own width restores the balance.
fn bar_frame(ctx: &egui::Context, separator_on: Edge) -> egui::Frame {
    let style = ctx.style();
    let base = egui::Frame::side_top_panel(&style);
    let line = style
        .visuals
        .widgets
        .noninteractive
        .bg_stroke
        .width
        .ceil()
        .max(0.0) as i8;

    let mut margin = base.inner_margin;
    match separator_on {
        Edge::Top => margin.top += line,
        Edge::Bottom => margin.bottom += line,
    }
    base.inner_margin(margin)
}

/// Breathing room between the canvas and the panels around it.
const CANVAS_MARGIN: i8 = 16;

impl eframe::App for NonogramGui {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.main_ui(ctx);
    }
}

impl NonogramGui {
    /// The whole window, panel by panel. Separate from `eframe::App::update` for testing.
    pub fn main_ui(&mut self, ctx: &egui::Context) {
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

        // Panel order matters: egui hands each panel the space its predecessors didn't claim, so
        // the top and bottom bars span the full width, and the sidebar then splits what's left
        // with the canvas.
        egui::TopBottomPanel::top("toolbar")
            .frame(bar_frame(ctx, Edge::Bottom))
            .show(ctx, |ui| {
                self.toolbar(ctx, ui);
            });

        egui::TopBottomPanel::bottom("status_bar")
            .frame(bar_frame(ctx, Edge::Top))
            .show(ctx, |ui| {
                // `editor_gui.status`/`editor_gui.progress` are shared (via `Rc<RefCell<_>>`) with
                // `solve_gui.canvas`, so this shows the latest message/progress regardless of which
                // mode is active.
                ui.horizontal(|ui| {
                    // Reserves a consistent height for the bar even when there's nothing to show,
                    // so the rest of the UI doesn't jump around as messages come and go.
                    ui.label("");

                    if let Some(progress) = *self.editor_gui.progress.borrow() {
                        // ~50% wider than the sidebar (150.0) is by default.
                        ui.add(
                            egui::ProgressBar::new(progress)
                                .animate(true)
                                .desired_width(225.0),
                        );
                    }

                    if let Some(status) = self.editor_gui.status.get() {
                        let color = if status.is_error {
                            Color32::DARK_RED
                        } else {
                            ui.visuals().text_color()
                        };
                        ui.colored_label(color, &status.text);
                    }
                });
            });

        egui::SidePanel::left("sidebar")
            .resizable(true)
            .default_width(SIDEBAR_WIDTH)
            .width_range(SIDEBAR_WIDTH..=400.0)
            .show(ctx, |ui| {
                // Both sidebars can outgrow a short window — the editor's once "Metadata" is
                // expanded, the solver's once a puzzle has a long palette.
                egui::ScrollArea::vertical().show(ui, |ui| {
                    if let Some(solve_gui) = &mut self.solve_gui {
                        solve_gui.sidebar(ui);
                    } else {
                        self.edit_sidebar(ui);
                    }
                });
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(&ctx.style()).inner_margin(CANVAS_MARGIN))
            .show(ctx, |ui| {
                // egui routes ctrl-scroll (and trackpad pinch) into `zoom_delta` rather than into
                // the scroll offset, so the scroll area below pans on a plain wheel and leaves
                // this alone. Only zoom when the pointer is actually over the canvas, so the
                // gesture doesn't fire while the user is over the sidebar.
                let zoom_here = ui.rect_contains_pointer(ui.max_rect());

                // A zoomed-in puzzle is routinely bigger than the window in both directions.
                egui::ScrollArea::both().show(ui, |ui| {
                    if let Some(solve_gui) = &mut self.solve_gui {
                        solve_gui.body(ui, self.scale);
                    } else {
                        self.editor_gui
                            .canvas(ui, self.scale, RenderStyle::Experimental);
                    }
                });

                if zoom_here {
                    let zoom = ui.input(|i| i.zoom_delta());
                    if zoom != 1.0 {
                        self.scale = (self.scale * zoom).clamp(1.0, 50.0);
                    }
                }
            });
    }
}

pub struct Disambiguator {
    /// Indexed by dense cell index, like `Solution::cells`.
    report: Option<Vec<(Color, f32)>>,
    pub terminate_s: mpsc::Sender<()>,
    progress_r: mpsc::Receiver<f32>,
    progress: f32,
    report_r: mpsc::Receiver<DisambigResult>,
}

impl Default for Disambiguator {
    fn default() -> Self {
        Self::new()
    }
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

    pub fn disambig_widget(
        &mut self,
        picture: &DynSolution,
        status: &SharedStatus,
        progress: &SharedProgress,
        ui: &mut egui::Ui,
    ) {
        while let Ok(p) = self.progress_r.try_recv() {
            self.progress = p;
        }
        let report_running = self.progress > 0.0 && self.progress < 1.0;

        // Taking the report before drawing means "Clear" becomes available on the same frame the
        // report lands, rather than the one after.
        if let Ok(result) = self.report_r.try_recv() {
            // Clear any stale message (e.g. a load error from before) now that disambiguation
            // has something new to say (or, for `Report`, nothing to say).
            status.maybe_clear_on_dirty();
            match result {
                DisambigResult::Unnecessary => {
                    status.set(StatusMessage::info("Disambiguation is unnecessary"));
                }
                DisambigResult::Report(report) => {
                    self.report = Some(report);
                }
            }
        }

        // Both buttons share a row: "Clear" discards what the button beside it produced. They
        // only record what was clicked, since acting on it needs `self` mutably.
        let (mut start, mut stop, mut clear) = (false, false, false);
        ui.horizontal(|ui| {
            if !report_running {
                start = ui.button("Disambiguate!").clicked();
            } else {
                stop = ui.button("Stop").clicked();
            }
            clear = ui
                .add_enabled(self.report.is_some(), egui::Button::new("Clear"))
                .clicked();
        });

        if start {
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
        if stop {
            let _ = self.terminate_s.send(()); // Don't panic if it's already gone!
            self.progress = 0.0;
        }

        *progress.borrow_mut() = if self.progress > 0.0 && self.progress < 1.0 {
            Some(self.progress)
        } else {
            None
        };

        if clear {
            self.report = None;
        }
    }
}
