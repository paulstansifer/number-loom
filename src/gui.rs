use std::{cell::RefCell, collections::HashMap, rc::Rc, sync::mpsc, time::Duration};

use web_time::Instant;

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum Tool {
    Pencil,
    FloodFill,
    LineAlongLane,
    /// Editor only — the solver's sidebar never offers it, so its canvas can't end up here.
    Lasso,
}

pub enum LibraryStatus {
    Loading,
    Loaded(Vec<Document>),
    Failed(String),
}

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
        if let Some((_, set_at)) = *inner {
            if set_at.elapsed() >= STATUS_GRACE_PERIOD {
                *inner = None;
            }
        }
    }
}

pub type SharedStatus = Rc<StatusCell>;

// Shared the same way as `SharedStatus`, for a progress bar in the status bar. `Some(fraction)`
// (0.0 to 1.0) while a long-running task is in progress, `None` otherwise.
pub type SharedProgress = Rc<RefCell<Option<f32>>>;

use crate::{
    export::to_bytes,
    grid_solve::{self, DisambigResult, disambig_candidates},
    gui_solver::{RenderStyle, SolveGui},
    import,
    // The abstract-units point, distinct from egui's `Pos2`: everything the lasso does is in
    // grid space, and only the painter converts.
    layout::Point,
    puzzle::{
        BACKGROUND, Clue, ClueStyle, Color, ColorInfo, Corner, Document, DynSolution, PuzzleDynOps,
        Solution, UNSOLVED,
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

/// A lasso selection, plus whatever it has been lifted onto a floating layer.
///
/// The mask is stored at an *anchor* position and displaced by `offset`, rather than being
/// rewritten on every drag frame: that keeps a move reversible (drag back off the grid edge and
/// nothing is lost) and makes the whole thing one translation away from where it started.
pub struct Selection {
    /// Indexed by dense cell index, like `Solution::cells` — so it is only meaningful for a grid
    /// of the same size, which `canvas_with_clues` checks before using it.
    mask: Vec<bool>,
    /// Lattice steps (see `Geometry::snap_translation`) currently applied to `mask` and
    /// `floating`. Zero until the selection is dragged.
    offset: (i32, i32),
    /// Content lifted off the grid, as `(anchor cell, color)`. `None` until the first move drag.
    floating: Option<Vec<(u32, Color)>>,
    /// The lasso path in abstract units, while one is being drawn.
    drawing: Option<Vec<crate::layout::Point>>,
    /// Where the move drag was grabbed, and the offset at that moment.
    dragging: Option<(crate::layout::Point, (i32, i32))>,
    /// Time origin for the marching ants' dash phase.
    since: Instant,
}

impl Selection {
    fn new(mask: Vec<bool>) -> Selection {
        Selection {
            mask,
            offset: (0, 0),
            floating: None,
            drawing: None,
            dragging: None,
            since: Instant::now(),
        }
    }

    fn anchor_cells(&self) -> impl Iterator<Item = u32> + '_ {
        self.mask
            .iter()
            .enumerate()
            .filter(|(_, m)| **m)
            .map(|(i, _)| i as u32)
    }

    /// Where the selected cells sit right now: the anchor mask shifted by `offset`. Cells pushed
    /// off the grid simply don't appear — they're still in `mask`, so dragging back restores them.
    fn displayed_cells(&self, picture: &crate::puzzle::DynSolution) -> Vec<u32> {
        if self.offset == (0, 0) {
            return self.anchor_cells().collect();
        }
        self.anchor_cells()
            .filter_map(|cell| picture.translate_cell(cell, self.offset))
            .collect()
    }

    fn is_empty(&self) -> bool {
        !self.mask.iter().any(|m| *m)
    }
}

/// What the lasso tool needs to know about the pointer in a frame.
#[derive(Clone, Copy, Debug, Default)]
struct LassoPointer {
    pressed: bool,
    down: bool,
    released: bool,
}

impl LassoPointer {
    fn from_egui(pointer: &egui::PointerState) -> LassoPointer {
        LassoPointer {
            pressed: pointer.button_pressed(egui::PointerButton::Primary),
            down: pointer.button_down(egui::PointerButton::Primary),
            released: pointer.any_released(),
        }
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
    /// Indexed by dense cell index, like `Solution::cells`.
    pub solved_mask: Staleable<(String, Vec<bool>)>,
    pub disambiguator: Staleable<Disambiguator>,
    pub id: Staleable<String>,
    pub status: SharedStatus,
    pub progress: SharedProgress,
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
                    self.document = document;
                    self.version += 1;
                    // A mask means nothing against a picture that was swapped out from under it,
                    // and a floating layer belongs to the picture it was lifted from.
                    self.selection = None;
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

        self.tool_selector(ui, editing);

        ui.separator();

        self.palette_editor(ui, palette_read_only);
    }

    fn tool_selector(&mut self, ui: &mut egui::Ui, editing: bool) {
        let was = self.current_tool;
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
                Tool::LineAlongLane,
                egui::RichText::new(icons::ICON_LINE_START).size(24.0),
            )
            .on_hover_text("Line along a row, column or diagonal");
            ui.selectable_value(
                &mut self.current_tool,
                Tool::FloodFill,
                egui::RichText::new(icons::ICON_FORMAT_COLOR_FILL).size(24.0),
            )
            .on_hover_text("Flood Fill");
            if editing {
                ui.selectable_value(
                    &mut self.current_tool,
                    Tool::Lasso,
                    egui::RichText::new(icons::ICON_LASSO_SELECT).size(24.0),
                )
                .on_hover_text("Lasso select: draw a loop, then drag to move what's inside");
            }
        });

        // Leaving the lasso commits whatever it was holding, so no other tool ever has to think
        // about a floating layer.
        if was == Tool::Lasso && self.current_tool != Tool::Lasso {
            self.clear_selection();
        }
    }

    fn flood_fill(&mut self, start: u32) {
        let picture = self.document.solution_mut();
        let target_color = picture.cells()[start as usize];
        if target_color == self.current_color {
            return; // Nothing to do
        }

        let mut changes = HashMap::new();
        let mut q = std::collections::VecDeque::new();
        let mut visited = std::collections::HashSet::new();

        q.push_back(start);
        visited.insert(start);

        while let Some(cell) = q.pop_front() {
            changes.insert(cell, self.current_color);

            // Adjacency comes from the geometry, so a triangle's three edge neighbours work
            // exactly as a square's four do.
            for neighbor in picture.neighbor_cells(cell) {
                if picture.cells()[neighbor as usize] == target_color && visited.insert(neighbor) {
                    q.push_back(neighbor);
                }
            }
        }

        if !changes.is_empty() {
            self.perform(Action::ChangeColor { changes }, ActionMood::Normal);
        }
    }

    /// Lift the selection onto a floating layer: remember what was there, then clear it to
    /// background. The area moved away from is always background, so this happens the moment a
    /// move begins rather than when it ends.
    ///
    /// Re-bases the mask onto wherever the selection is showing right now, so `offset` measures
    /// the move from here and the whole thing runs at most once per selection.
    fn lift_selection(&mut self) {
        let Some(selection) = &mut self.selection else {
            return;
        };
        if selection.floating.is_some() {
            return; // Already lifted; a second drag just changes the offset.
        }

        let picture = self.document.solution_mut();
        let displayed = selection.displayed_cells(picture);
        let cells = picture.cells();

        let mut mask = vec![false; cells.len()];
        let mut floating = Vec::with_capacity(displayed.len());
        for cell in displayed {
            mask[cell as usize] = true;
            floating.push((cell, cells[cell as usize]));
        }

        selection.mask = mask;
        selection.offset = (0, 0);
        selection.floating = Some(floating);

        let changes: HashMap<u32, Color> = selection
            .floating
            .as_ref()
            .unwrap()
            .iter()
            .map(|(cell, _)| (*cell, BACKGROUND))
            .collect();
        self.perform(Action::ChangeColor { changes }, ActionMood::Normal);
    }

    /// Stamp the floating layer back into the picture at wherever it has been dragged to, and
    /// re-base the selection there. The selection itself survives — only its floating-ness ends.
    ///
    /// Only non-background cells are stamped, so a moved shape composites over what's already
    /// there instead of punching a background-coloured hole around itself. Cells that have been
    /// dragged off the grid are dropped here — this is the point of no return for them.
    pub fn flatten_selection(&mut self) {
        let Some(selection) = &mut self.selection else {
            return;
        };
        let Some(floating) = selection.floating.take() else {
            return;
        };

        let picture = self.document.solution_mut();
        let mut mask = vec![false; picture.cells().len()];
        let mut changes = HashMap::new();
        for (cell, color) in floating {
            let Some(dest) = picture.translate_cell(cell, selection.offset) else {
                continue;
            };
            mask[dest as usize] = true;
            if color != BACKGROUND {
                changes.insert(dest, color);
            }
        }

        selection.mask = mask;
        selection.offset = (0, 0);

        if !changes.is_empty() {
            self.perform(Action::ChangeColor { changes }, ActionMood::Normal);
        }
    }

    /// Flatten whatever is floating and forget the selection entirely.
    pub fn clear_selection(&mut self) {
        self.flatten_selection();
        self.selection = None;
    }

    /// Fill the selection with background, without moving anything.
    fn erase_selection(&mut self) {
        let Some(selection) = &self.selection else {
            return;
        };
        let picture = self.document.solution_mut();
        let changes: HashMap<u32, Color> = selection
            .displayed_cells(picture)
            .into_iter()
            .map(|cell| (cell, BACKGROUND))
            .collect();
        if !changes.is_empty() {
            self.perform(Action::ChangeColor { changes }, ActionMood::Normal);
        }
    }

    /// Whether the point is inside the selection as it's currently displayed — i.e. whether
    /// pressing there starts a move rather than a new lasso.
    fn selection_contains(&mut self, p: Point) -> bool {
        let picture = self.document.solution_mut();
        let Some(cell) = picture.cell_at(p).and_then(|c| picture.cell_of(c)) else {
            return false;
        };
        self.cell_is_selected(cell)
    }

    /// One frame of lasso pointer input, at `p` in abstract units.
    ///
    /// Takes the three facts it needs rather than egui's `PointerState` so the whole
    /// draw-drag-flatten cycle can be driven from a test without a window.
    fn lasso_input(&mut self, pointer: LassoPointer, p: Point) {
        // Secondary and middle buttons paint in the other tools; here they'd have no meaning, so
        // they're simply ignored rather than doing something surprising.
        if pointer.pressed {
            if self.selection_contains(p) {
                self.lift_selection();
                let offset = self.selection.as_ref().map_or((0, 0), |s| s.offset);
                if let Some(selection) = &mut self.selection {
                    selection.dragging = Some((p, offset));
                }
            } else {
                self.flatten_selection();
                self.selection = None;
                let mut selection =
                    Selection::new(vec![false; self.document.solution_mut().cells().len()]);
                selection.drawing = Some(vec![p]);
                self.selection = Some(selection);
            }
        } else if pointer.down {
            let snapped =
                self.selection
                    .as_ref()
                    .and_then(|s| s.dragging)
                    .map(|(grab, offset_at_grab)| {
                        let by = crate::layout::Vec2::new(p.x - grab.x, p.y - grab.y);
                        let (du, dv) = self.document.solution_mut().snap_translation(by);
                        (offset_at_grab.0 + du, offset_at_grab.1 + dv)
                    });
            if let Some(selection) = &mut self.selection {
                if let Some(offset) = snapped {
                    // The lattice map is linear, so snapping the displacement and adding steps
                    // gives the same answer as snapping the total.
                    selection.offset = offset;
                } else if let Some(path) = &mut selection.drawing {
                    // Thin the path as it's drawn: the rasterizer's cost is linear in its length,
                    // and points a tenth of a cell apart tell us nothing new.
                    let last = path.last().copied().unwrap_or(p);
                    if (p.x - last.x).hypot(p.y - last.y) >= 0.1 || path.len() == 1 {
                        path.push(p);
                    }
                }
            }
        } else if pointer.released {
            let path = self.selection.as_mut().and_then(|selection| {
                selection.dragging = None;
                selection.drawing.take()
            });
            if let Some(path) = path {
                let mask = cells_in_lasso(self.document.solution_mut(), &path);
                if let Some(selection) = &mut self.selection {
                    selection.mask = mask;
                    selection.since = Instant::now();
                }
                if self.selection.as_ref().is_some_and(|s| s.is_empty()) {
                    self.selection = None; // A stray click shouldn't leave an invisible selection.
                }
            }
        }
    }

    /// Escape drops the selection; Delete/Backspace clears it to background. Read raw, matching
    /// how the undo/redo shortcuts alongside the toolbar already work.
    fn lasso_keys(&mut self, ui: &egui::Ui) {
        if self.selection.is_none() {
            return;
        }
        let (escape, delete) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::Escape),
                i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace),
            )
        });
        if escape {
            self.clear_selection();
        } else if delete {
            self.erase_selection();
        }
    }

    /// The traditional four-arrow cursor over the selection says "this can be dragged"; the
    /// crosshair elsewhere says "this draws a loop".
    fn lasso_cursor(&mut self, ui: &egui::Ui, hovered_cell: Option<u32>) {
        // Not over the grid at all — leave the cursor to whatever else is under it.
        let Some(cell) = hovered_cell else {
            return;
        };
        let dragging = self
            .selection
            .as_ref()
            .is_some_and(|s| s.dragging.is_some());
        let icon = if dragging || self.cell_is_selected(cell) {
            egui::CursorIcon::Move
        } else {
            egui::CursorIcon::Crosshair
        };
        ui.ctx().set_cursor_icon(icon);
    }

    /// Whether a cell is part of the selection as displayed. Asks where the cell *came from*
    /// rather than materializing the whole displaced set.
    fn cell_is_selected(&mut self, cell: u32) -> bool {
        let Some(selection) = &self.selection else {
            return false;
        };
        if selection.drawing.is_some() {
            return false;
        }
        let (u, v) = selection.offset;
        let anchor = self.document.solution_mut().translate_cell(cell, (-u, -v));
        let selection = self.selection.as_ref().unwrap();
        anchor.is_some_and(|anchor| selection.mask[anchor as usize])
    }
}

/// How finely the lasso path is sampled when marking the cells it passes through, in abstract
/// units. Must be below the smallest cell dimension — a triangle row is only `TRI_ROW_HEIGHT`
/// (0.87) tall and cells are half a base wide — so that no cell the path crosses is stepped over.
const LASSO_STEP: f32 = 0.2;

/// The cells a closed lasso path touches or encloses.
///
/// "Touched" is found by walking the path rather than by remembering which cells the pointer was
/// over: that covers the straight closing segment and fast drags that skip cells between frames,
/// with one rule instead of three. "Enclosed" is an even-odd ray cast against each cell's
/// centroid. Runs once, on release, so `cells × path points` is fine.
pub fn cells_in_lasso(picture: &crate::puzzle::DynSolution, path: &[Point]) -> Vec<bool> {
    let mut mask = vec![false; picture.cells().len()];
    if path.len() < 2 {
        // A click rather than a drag: just the cell under it, if any.
        if let Some(cell) = path
            .first()
            .and_then(|p| picture.cell_at(*p))
            .and_then(|c| picture.cell_of(c))
        {
            mask[cell as usize] = true;
        }
        return mask;
    }

    // Touched, including along the closing segment from the last point back to the first.
    for (a, b) in path
        .iter()
        .zip(path.iter().cycle().skip(1))
        .take(path.len())
    {
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let steps = ((dx.hypot(dy) / LASSO_STEP).ceil() as usize).max(1);
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            let p = Point::new(a.x + dx * t, a.y + dy * t);
            if let Some(cell) = picture.cell_at(p).and_then(|c| picture.cell_of(c)) {
                mask[cell as usize] = true;
            }
        }
    }

    // Enclosed: a horizontal ray from the centroid crosses the closed path an odd number of times.
    for cell in 0..mask.len() as u32 {
        if mask[cell as usize] {
            continue;
        }
        let c = picture.cell_shape(cell).center(picture.cell_origin(cell));
        let mut inside = false;
        for (a, b) in path
            .iter()
            .zip(path.iter().cycle().skip(1))
            .take(path.len())
        {
            if (a.y > c.y) != (b.y > c.y) && c.x < a.x + (c.y - a.y) / (b.y - a.y) * (b.x - a.x) {
                inside = !inside;
            }
        }
        mask[cell as usize] = inside;
    }

    mask
}

/// What a canvas needs in order to draw clue gutters around the picture. Only the solve view
/// supplies this; the editor draws the picture alone.
pub struct ClueOverlay<'a> {
    pub puzzle: &'a crate::puzzle::DynPuzzle,
    /// One `Vec<LineStatus>` per clue family, in family order.
    pub analysis: Option<&'a Vec<Vec<crate::grid_solve::LineStatus>>>,
    pub is_stale: bool,
}

impl CanvasGui {
    /// How far each lane's clues reach out from the grid, in abstract units.
    fn clue_run_length(puzzle: &crate::puzzle::DynPuzzle, lane: usize) -> f32 {
        let parts = crate::with_puzzle!(puzzle, |p| {
            p.lines[lane]
                .iter()
                .map(|c| c.express(&p.palette).len())
                .sum::<usize>()
        });
        crate::layout::GutterLane::clue_run_length(parts)
    }

    /// Draw the picture and handle pointer input. Returns the hovered cell, if any.
    ///
    /// Shape-specific work happens in exactly two places: the hit test, and the render loop.
    /// Everything else — the tools, undo, the overlays — works in dense cell indices and is the
    /// same for every shape.
    pub fn canvas(
        &mut self,
        ui: &mut egui::Ui,
        scale: f32,
        render_style: RenderStyle,
    ) -> Option<u32> {
        self.canvas_with_clues(ui, scale, render_style, None)
    }

    /// As `canvas`, but reserving room around the picture for clue gutters and drawing them.
    ///
    /// Clues share the picture's painter and coordinate system rather than living in their own
    /// widgets, because a hexagon's three clue blocks are not axis-aligned rectangles and can't be
    /// laid out by a grid of separate panels.
    pub fn canvas_with_clues(
        &mut self,
        ui: &mut egui::Ui,
        scale: f32,
        render_style: RenderStyle,
        clues: Option<ClueOverlay<'_>>,
    ) -> Option<u32> {
        let extent = self.document.solution_mut().extent();

        // Grow the drawing area to cover wherever the clues reach.
        let (mut lo, mut hi) = (
            crate::layout::Point::new(0.0, 0.0),
            crate::layout::Point::new(extent.x, extent.y),
        );
        if let Some(overlay) = &clues {
            for (_, gutter) in self.document.solution_mut().gutters() {
                for g in gutter {
                    let len = Self::clue_run_length(overlay.puzzle, g.lane);
                    let tip = crate::layout::Point::new(
                        g.anchor.x + g.outward.x * len,
                        g.anchor.y + g.outward.y * len,
                    );
                    let half = crate::layout::CLUE_BOX;
                    lo.x = lo.x.min(tip.x - half);
                    lo.y = lo.y.min(tip.y - half);
                    hi.x = hi.x.max(tip.x + half);
                    hi.y = hi.y.max(tip.y + half);
                }
            }
        }
        let full = Vec2::new(hi.x - lo.x, hi.y - lo.y);

        let (mut response, painter) = ui.allocate_painter(
            Vec2::new(scale * full.x, scale * full.y) + Vec2::new(2.0, 2.0), // for the border
            egui::Sense::click_and_drag(),
        );

        let canvas_without_border = response.rect.shrink(1.0);

        // One abstract unit is one cell edge, so this is a plain uniform scale. `lo` is where the
        // outermost clue sits, so the picture itself is offset by `-lo`.
        let to_screen = egui::emath::RectTransform::from_to(
            Rect::from_min_size(Pos2::new(lo.x, lo.y), full),
            canvas_without_border,
        );
        let from_screen = to_screen.inverse();

        let cell_under = |picture: &crate::puzzle::DynSolution, pos: Pos2| -> Option<u32> {
            let p = from_screen * pos;
            picture
                .cell_at(crate::layout::Point::new(p.x, p.y))
                .and_then(|coord| picture.cell_of(coord))
        };

        let hovered_cell = response
            .hover_pos()
            .and_then(|pos| cell_under(self.document.solution_mut(), pos));

        // A mask is a dense-cell-index array, so it means nothing once the grid is a different
        // size. Checking here rather than at every resize/load site means there's no call site
        // to forget.
        if let Some(selection) = &self.selection
            && selection.mask.len() != self.document.solution_mut().cells().len()
        {
            self.selection = None;
        }

        if self.current_tool == Tool::Lasso {
            // The lasso is the one tool that must keep tracking the pointer once it leaves the
            // grid — a loop drawn around the outside of a shape is perfectly ordinary — so it
            // works from the abstract-unit position directly, not from a cell.
            if let Some(pointer_pos) = response.interact_pointer_pos() {
                let p = from_screen * pointer_pos;
                let pointer = LassoPointer::from_egui(&ui.input(|i| i.pointer.clone()));
                self.lasso_input(pointer, Point::new(p.x, p.y));
            }
            self.lasso_keys(ui);
            self.lasso_cursor(ui, hovered_cell);
        }

        if let Some(pointer_pos) = response.interact_pointer_pos() {
            let picture = self.document.solution_mut();
            if let Some(cell) = cell_under(picture, pointer_pos).filter(|_| {
                // Handled above, without needing a cell.
                self.current_tool != Tool::Lasso
            }) {
                let pointer = &ui.input(|i| i.pointer.clone());
                let paint_color = if pointer.middle_down() {
                    if picture.palette().contains_key(&UNSOLVED) {
                        UNSOLVED
                    } else {
                        BACKGROUND
                    }
                } else if pointer.secondary_down() {
                    BACKGROUND
                } else if picture.cells()[cell as usize] != self.current_color {
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

                        self.perform(
                            Action::ChangeColor {
                                changes: [(cell, self.drag_start_color)].into(),
                            },
                            mood,
                        );
                    }
                    Tool::FloodFill => {
                        if pointer.any_click() {
                            let original_color = self.current_color;
                            self.current_color = paint_color;
                            self.flood_fill(cell);
                            self.current_color = original_color;
                        }
                    }
                    Tool::LineAlongLane => {
                        if pointer.any_pressed() {
                            self.drag_start_color = paint_color;
                            self.line_tool_state = Some(cell);

                            self.perform(
                                Action::ChangeColor {
                                    changes: [(cell, self.drag_start_color)].into(),
                                },
                                ActionMood::Normal,
                            );
                        } else if pointer.any_down() {
                            if let Some(start) = self.line_tool_state {
                                let changes = self.line_between(start, cell);
                                self.perform(
                                    Action::ChangeColor { changes },
                                    ActionMood::ReplaceAction,
                                );
                            }
                        } else if pointer.any_released() {
                            self.line_tool_state = None;
                        }
                    }
                    // Handled above, where the pointer is still allowed to be off the grid.
                    Tool::Lasso => {}
                }
            }
        }

        let mut shapes = vec![];
        let disambiguator = self.disambiguator.get_if_fresh(self.version);
        let disambig_report = disambiguator.as_ref().and_then(|d| d.report.as_ref());
        let solved_mask = self.solved_mask.get_if_fresh(self.version);
        let overlays_suppress_unsolved = disambig_report.is_some()
            || disambiguator.map_or(false, |d| d.progress > 0.0 && d.progress < 1.0);

        let picture = self.document.try_solution().unwrap();
        let palette = picture.palette();

        // The one place the shape matters when drawing. After this match the loop is fully
        // monomorphized: the inner iterator just walks a slice and advances an `f32`.
        crate::with_solution!(picture, |sol| {
            for row in sol.geometry.rows() {
                for drawn in row.cells() {
                    let index = drawn.cell as usize;
                    let color_info = &palette[&sol.cells[index]];
                    let solved =
                        solved_mask.map_or(true, |sm| sm.1[index]) || overlays_suppress_unsolved;
                    let mut dr = (&palette[&BACKGROUND], 1.0);
                    if let Some(report) = disambig_report.as_ref() {
                        let (c, score) = report[index];
                        dr = (&palette[&c], score);
                    }
                    shapes.extend(cell_shape(
                        color_info,
                        solved,
                        dr,
                        drawn.shape,
                        drawn.origin,
                        &to_screen,
                        render_style,
                    ));
                }
            }
        });

        // The floating layer, drawn on top of the picture at wherever it's been dragged to. The
        // cells it was lifted from already read as background, so this is the only thing standing
        // between the two positions.
        if let Some(selection) = &self.selection
            && let Some(floating) = &selection.floating
        {
            for (cell, color) in floating {
                // Background cells move transparently: the shape composites over whatever it's
                // dropped onto instead of carrying a background-coloured box with it.
                if *color == BACKGROUND {
                    continue;
                }
                let Some(dest) = picture.translate_cell(*cell, selection.offset) else {
                    continue; // Dragged off the grid; still in the layer, just not visible.
                };
                shapes.extend(cell_shape(
                    &palette[color],
                    true,
                    (&palette[&BACKGROUND], 1.0),
                    picture.cell_shape(dest),
                    picture.cell_origin(dest),
                    &to_screen,
                    render_style,
                ));
            }
        }

        // Clue gutters, in the same coordinate system as the picture.
        if let Some(overlay) = &clues {
            let lane_families: Vec<usize> = picture
                .lane_map()
                .lanes()
                .iter()
                .map(|l| l.family)
                .collect();
            let family_starts: Vec<usize> = (0..picture.lane_map().family_count())
                .map(|f| picture.lane_map().family(f).start)
                .collect();

            for (_, gutter) in picture.gutters() {
                for g in gutter {
                    let expressed = crate::with_puzzle!(overlay.puzzle, |p| {
                        let mut v: Vec<(ColorInfo, Option<u16>)> = p.lines[g.lane]
                            .iter()
                            .flat_map(|c| {
                                c.express(&p.palette)
                                    .into_iter()
                                    .map(|(ci, n)| (ci.clone(), n))
                            })
                            .collect();
                        // Clues run in the lane's own direction, so the box nearest the grid is
                        // the last one; `reversed` covers the families whose clues are labelled
                        // at the far end from where the lane is stored.
                        if !g.reversed {
                            v.reverse();
                        }
                        v
                    });

                    let family = lane_families[g.lane];
                    for (i, (color_info, count)) in expressed.iter().enumerate() {
                        let c = g.clue_box_center(i);
                        let points = crate::layout::tri_clue_rhombus(
                            c,
                            family,
                            g.edge_dir,
                            crate::layout::CLUE_BOX,
                            crate::layout::CLUE_BOX_SHORT,
                        )
                        .map(|p| to_screen * Pos2::new(p.x, p.y));
                        let text = match count {
                            Some(n) => n.to_string(),
                            None => color_info.ch.to_string(),
                        };
                        crate::gui_solver::draw_string_in_rhombus(
                            ui,
                            &painter,
                            &points,
                            &text,
                            scale,
                            color_info.rgb,
                        );
                    }

                    // The analysis mark sits between the clues and the grid.
                    if let Some(analysis) = overlay.analysis {
                        let family = lane_families[g.lane];
                        let index = g.lane - family_starts[family];
                        if let Some(status) = analysis.get(family).and_then(|f| f.get(index)) {
                            let at = to_screen
                                * Pos2::new(
                                    g.anchor.x + g.outward.x * (crate::layout::CLUE_PAD / 2.0),
                                    g.anchor.y + g.outward.y * (crate::layout::CLUE_PAD / 2.0),
                                );
                            crate::gui_solver::draw_analysis_mark(
                                &painter,
                                at,
                                scale,
                                status,
                                overlay.is_stale,
                            );
                        }
                    }
                }
            }
        }

        // Grid lines, precomputed by the geometry: one boundary per lane, with every fifth one
        // heavier — which for a triddler means every fifth lane *within a family*.
        for guide in picture.guides() {
            let points = [
                to_screen * Pos2::new(guide.from.x, guide.from.y),
                to_screen * Pos2::new(guide.to.x, guide.to.y),
            ];
            let stroke = egui::Stroke::new(
                1.0,
                egui::Color32::from_black_alpha(if guide.emphasis { 64 } else { 16 }),
            );
            shapes.push(egui::Shape::line_segment(points, stroke));
        }

        if let Some(selection) = &self.selection {
            if let Some(path) = &selection.drawing {
                // The loop as it's being drawn, closed so the player can see what they'll get.
                let points: Vec<Pos2> = path
                    .iter()
                    .chain(path.first())
                    .map(|p| to_screen * Pos2::new(p.x, p.y))
                    .collect();
                shapes.push(egui::Shape::line(
                    points,
                    egui::Stroke::new(1.0, Color32::from_black_alpha(160)),
                ));
            } else {
                shapes.extend(marching_ants(
                    &selection_outline(picture, &selection.displayed_cells(picture)),
                    &to_screen,
                    selection.since.elapsed().as_secs_f32(),
                ));
            }
            // Only while a selection exists, so the idle app stays idle.
            ui.ctx().request_repaint_after(Duration::from_millis(50));
        }

        painter.extend(shapes);
        response.mark_changed();

        hovered_cell
    }

    /// The cells between two points along whichever lane best matches the drag.
    ///
    /// A square grid offers two directions through a cell; a triddler offers three. Picking the
    /// family whose lane actually contains both endpoints generalizes the old "is this drag more
    /// horizontal than vertical?" test.
    fn line_between(&mut self, start: u32, end: u32) -> HashMap<u32, Color> {
        let picture = self.document.solution_mut();
        let lanes = picture.lane_map();

        let mut best: Option<(usize, usize, usize)> = None; // (lane, from, to)
        for membership in lanes.memberships(start) {
            let cells = &lanes.lane(membership.lane as usize).cells;
            if let Some(end_pos) = cells.iter().position(|c| *c == end) {
                let from = (membership.position as usize).min(end_pos);
                let to = (membership.position as usize).max(end_pos);
                // Prefer the longest run, so a drag along a lane wins over a one-cell overlap
                // with a lane that merely happens to touch both ends.
                if best.map_or(true, |(_, bf, bt)| to - from > bt - bf) {
                    best = Some((membership.lane as usize, from, to));
                }
            }
        }

        let mut changes = HashMap::new();
        match best {
            Some((lane, from, to)) => {
                for cell in &lanes.lane(lane).cells[from..=to] {
                    changes.insert(*cell, self.drag_start_color);
                }
            }
            // No lane joins them, so just paint the endpoint.
            None => {
                changes.insert(end, self.drag_start_color);
            }
        }
        changes
    }

    fn palette_editor(&mut self, ui: &mut egui::Ui, read_only: bool) {
        let mut picked_color = self.current_color;
        let mut removed_color = None;
        let mut add_color = false;

        use itertools::Itertools;

        for (color, color_info) in self
            .document
            .solution_mut()
            .palette_mut()
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
                if ui.add(egui::Button::new(color_text)).clicked() {
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
            for cell in new_picture.cells_mut().iter_mut() {
                if *cell == removed_color {
                    *cell = self.current_color;
                }
            }
            new_picture.palette_mut().remove(&removed_color);
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
            let next_color = Color(new_picture.palette().keys().map(|k| k.0).max().unwrap() + 1);
            new_picture.palette_mut().insert(
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

/// The outline of a set of cells, as a list of abstract-unit segments.
///
/// Found by cancellation rather than by asking which neighbour lies across which edge: push every
/// selected cell's edges into a table, and an edge shared by two selected cells lands there twice.
/// What's left having landed once is exactly the boundary. That needs nothing shape-specific, so
/// squares and both triangle orientations come out right with no dispatch.
fn selection_outline(picture: &DynSolution, cells: &[u32]) -> Vec<(Point, Point)> {
    /// A cell corner quantized onto a fixed sub-cell grid. Corners land on exact lattice values,
    /// so this is stable, and two cells' shared edge always produces the identical key.
    type Vertex = (i32, i32);
    /// An edge, as its two vertices in a canonical order.
    type EdgeKey = (Vertex, Vertex);

    let key =
        |p: Point| -> Vertex { ((p.x * 4096.0).round() as i32, (p.y * 4096.0).round() as i32) };

    let mut edges: HashMap<EdgeKey, ((Point, Point), u32)> = HashMap::new();
    for cell in cells {
        let shape = picture.cell_shape(*cell);
        let (points, n) = shape.vertices(picture.cell_origin(*cell));
        for i in 0..n {
            let (a, b) = (points[i], points[(i + 1) % n]);
            let (ka, kb) = (key(a), key(b));
            let k = if ka <= kb { (ka, kb) } else { (kb, ka) };
            edges.entry(k).or_insert(((a, b), 0)).1 += 1;
        }
    }

    edges
        .into_values()
        .filter(|(_, count)| *count == 1)
        .map(|(edge, _)| edge)
        .collect()
}

/// Dash length and gap for the marching ants, in points, and how fast the dashes crawl.
const ANT_DASH: f32 = 4.0;
const ANT_SPEED: f32 = 12.0;

/// Draw a selection outline as marching ants: dark dashes crawling over a light line, so the
/// outline reads against both a filled cell and an empty one.
fn marching_ants(
    outline: &[(Point, Point)],
    to_screen: &egui::emath::RectTransform,
    elapsed: f32,
) -> Vec<Shape> {
    let mut shapes = Vec::with_capacity(outline.len() * 2);
    let offset = -(elapsed * ANT_SPEED) % (ANT_DASH * 2.0);

    for (a, b) in outline {
        let points = [
            to_screen * Pos2::new(a.x, a.y),
            to_screen * Pos2::new(b.x, b.y),
        ];
        shapes.push(Shape::line_segment(
            points,
            egui::Stroke::new(1.5, Color32::from_white_alpha(220)),
        ));
        shapes.extend(Shape::dashed_line_with_offset(
            &points,
            egui::Stroke::new(1.5, Color32::from_black_alpha(220)),
            &[ANT_DASH],
            &[ANT_DASH],
            offset,
        ));
    }
    shapes
}

/// Build the shapes for one cell. `shape` and `origin` come from the geometry, so a triangle is
/// drawn as a triangle and every overlay lands on the real centroid rather than the middle of a
/// bounding box.
fn cell_shape(
    ci: &ColorInfo,
    solved: bool,
    disambig: (&ColorInfo, f32),
    shape: crate::layout::CellShape,
    origin: crate::layout::Point,
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

    let screen = |p: crate::layout::Point| to_screen * Pos2::new(p.x, p.y);
    let polygon = |(points, n): ([crate::layout::Point; 4], usize), fill| {
        egui::Shape::convex_polygon(
            points[..n].iter().map(|p| screen(*p)).collect(),
            fill,
            egui::Stroke::default(),
        )
    };

    // A `Corner` colour is a half-square used by trianogram clues, which is a different thing
    // from a triangular *cell* and only ever appears on a square grid.
    let mut res = vec![match ci.corner {
        None => polygon(shape.vertices(origin), color),
        Some(corner) => {
            let mut half = triangle_shape(corner, color, to_screen.scale());
            half.translate(screen(origin).to_vec2());
            half
        }
    }];

    let center = screen(shape.center(origin));
    let unit = to_screen.scale().x;

    if ci.color == BACKGROUND {
        match render_style {
            RenderStyle::TraditionalDots => {
                res.push(egui::Shape::circle_filled(
                    center,
                    unit * 0.1,
                    egui::Color32::from_rgb(190, 190, 190),
                ));
            }
            RenderStyle::TraditionalXes => {
                let stroke = egui::Stroke::new(2.0, Color32::from_rgb(190, 190, 190));
                let radius = unit * 0.2;
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
        res.push(polygon(
            shape.shrunk(origin, 0.6),
            egui::Color32::from_rgb(230, 230, 230),
        ));
    }

    if !solved {
        res.push(egui::Shape::circle_filled(
            center,
            unit * 0.3,
            egui::Color32::from_rgb(190, 190, 190),
        ))
    }

    if disambig.1 < 1.0 {
        let (r, g, b) = disambig.0.rgb;
        res.push(polygon(
            shape.shrunk(origin, 0.5),
            Color32::from_rgba_unmultiplied(r, g, b, ((1.0 - disambig.1) * 255.0) as u8),
        ));
    }

    res
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

        let mut current_color = BACKGROUND;
        if picture.palette().contains_key(&Color(1)) {
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
                selection: None,
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
            auto_solve: false,
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

    fn resize(&mut self, top: Option<bool>, left: Option<bool>, add: bool) {
        // This resizer is inherently square: it adds and removes whole rows and columns.
        // Triangular puzzles resize by nudging one of six bounds instead (see
        // `Outline::resized` and `Geometry::resized`), which needs its own UI.
        let Some(picture) = self.editor_gui.document.square_solution_mut() else {
            self.editor_gui
                .status
                .set(StatusMessage::error(TRIDDLER_UNSUPPORTED));
            return;
        };
        let mut g = picture.to_columns();
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
        {
            let solution = new_doc.solution_mut();
            *solution = DynSolution::Square(Solution::from_columns(
                solution.clue_style(),
                solution.palette().clone(),
                g,
            ));
        }
        self.editor_gui.perform(
            Action::ReplaceDocument { document: new_doc },
            ActionMood::Normal,
        );
    }

    fn resizer(&mut self, ui: &mut egui::Ui) {
        let (width, height) = self.editor_gui.document.dimensions();
        ui.label(format!("Canvas size: {}x{}", width, height));

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

            self.editor_gui.common_sidebar_items(ui, false, true);

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

            ui.separator();

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
                    .add_filter("woven", &["woven"])
                    .pick_file()
                    .await;

                if let Some(handle) = handle {
                    let document =
                        crate::import::load(&handle.file_name(), handle.read().await, None);

                    sender.send(document).unwrap();
                }
            });
        }

        if let Ok(result) = self.opened_file_receiver.try_recv() {
            match result {
                Ok(document) => {
                    self.editor_gui
                        .perform(Action::ReplaceDocument { document }, ActionMood::Normal);
                }
                Err(e) => {
                    self.editor_gui
                        .status
                        .set(StatusMessage::error(format!("Error loading file: {:?}", e)));
                }
            }
        }
    }

    fn enter_solve_mode(&mut self) {
        self.solve_mode = true;

        self.solve_gui = Some(crate::gui_solver::SolveGui::new(
            self.editor_gui.document.clone(),
            Rc::clone(&self.editor_gui.status),
            Rc::clone(&self.editor_gui.progress),
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
            if ui.button("New").clicked() {
                let (x_size, y_size) = self.editor_gui.document.dimensions();
                self.new_dialog = Some(NewPuzzleDialog {
                    shape: NewPuzzleShape::Square,
                    clue_style: self.editor_gui.document.solution_mut().clue_style(),
                    x_size,
                    y_size,
                    tri_side: 3,
                });
            }
            let mut new_document = None;
            if let Some(dialog) = self.new_dialog.as_mut() {
                egui::Window::new("New puzzle").show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut dialog.shape, NewPuzzleShape::Square, "Square");
                        ui.radio_value(&mut dialog.shape, NewPuzzleShape::Triangular, "Triddler");
                    });

                    match dialog.shape {
                        NewPuzzleShape::Square => {
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
                        }
                        NewPuzzleShape::Triangular => {
                            // Trianogram clues on a triddler are rejected at construction, so
                            // there's nothing to choose here — a triddler is always a nonogram.
                            ui.add(
                                egui::Slider::new(&mut dialog.tri_side, 1..=10)
                                    .text("hexagon side"),
                            );
                        }
                    }

                    if ui.button("Ok").clicked() {
                        let new_solution = match dialog.shape {
                            NewPuzzleShape::Square => DynSolution::Square(Solution::new(
                                dialog.clue_style,
                                match dialog.clue_style {
                                    ClueStyle::Nono => import::bw_palette(),
                                    ClueStyle::Triano => import::triano_palette(),
                                },
                                crate::geometry::Geometry::new(crate::geometry::Rect {
                                    width: dialog.x_size,
                                    height: dialog.y_size,
                                }),
                                vec![BACKGROUND; dialog.x_size * dialog.y_size],
                            )),
                            NewPuzzleShape::Triangular => {
                                let geometry = crate::geometry::Geometry::new(
                                    crate::geometry::Outline::hexagon(dialog.tri_side),
                                );
                                let cells = vec![BACKGROUND; geometry.cell_count()];
                                DynSolution::Tri(Solution::new(
                                    ClueStyle::Nono,
                                    import::bw_palette(),
                                    geometry,
                                    cells,
                                ))
                            }
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
                self.library_dialog = Some(LibraryStatus::Loading);

                spawn_async(async move {
                    let result = crate::import::puzzles_from_github().await;
                    let _ = sender.send(result);
                });
            }

            if let Ok(result) = self.library_receiver.try_recv() {
                match result {
                    Ok(library) => self.library_dialog = Some(LibraryStatus::Loaded(library)),
                    Err(e) => self.library_dialog = Some(LibraryStatus::Failed(e.to_string())),
                }
            }

            let mut next_enter_solve_mode = false;
            let mut close_library = false;
            if let Some(status) = &self.library_dialog {
                egui::Window::new("Puzzle Library")
                    .max_size(ctx.screen_rect().size() * 0.9)
                    .show(ctx, |ui| {
                        match status {
                            LibraryStatus::Loading => {
                                ui.vertical_centered(|ui| {
                                    ui.add(egui::Spinner::new());
                                    ui.label("Loading library...");
                                });
                            }
                            LibraryStatus::Loaded(docs) => {
                                egui::ScrollArea::vertical().show(ui, |ui| {
                                    egui::Grid::new("library_grid").show(ui, |ui| {
                                        for (i, doc) in docs.iter().enumerate() {
                                            if crate::gui_gallery::gallery_puzzle_preview(ui, doc)
                                                .clicked()
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
                            }
                            LibraryStatus::Failed(e) => {
                                ui.vertical_centered(|ui| {
                                    ui.label(
                                        RichText::new(format!("Failed to load library: {}", e))
                                            .color(Color32::RED),
                                    );
                                });
                            }
                        }
                        ui.separator();
                        if ui.button("Cancel").clicked() {
                            close_library = true;
                        }
                    });
            }
            if close_library {
                self.library_dialog = None;
            }
            self.loader(ui);

            if ui.button("Save/share").clicked() {
                self.share_string =
                    crate::formats::woven::to_woven(&mut self.editor_gui.document).unwrap();
                self.quality_warnings = self.editor_gui.document.quality_check();
                self.show_save_share_window = true;
            }

            if self.show_save_share_window {
                egui::Window::new("Save/share")
                    .open(&mut self.show_save_share_window)
                    .default_width(780.0)
                    .show(ctx, |ui| {
                        if !self.quality_warnings.is_empty() {
                            if self.quality_warnings.len() == 1 {
                                ui.label("Warning:");
                            } else {
                                ui.label("Warnings:");
                            }
                            for warning in &self.quality_warnings {
                                ui.label(warning);
                            }
                            ui.separator();
                        }
                        ui.label("Share String:");
                        ui.add(
                            egui::TextEdit::multiline(&mut self.share_string.clone())
                                .font(TextStyle::Monospace)
                                .desired_width(730.0),
                        );
                        if ui.button("Copy to clipboard").clicked() {
                            ctx.copy_text(self.share_string.clone());
                        }

                        if self.editor_gui.document.license == "CC BY 4.0" {
                            if self.editor_gui.document.author.trim().is_empty() {
                                ui.label(
                                    "(The author field in your puzzle is empty; please use \
                                    'anonymous' if that's what you want.)",
                                );
                            }
                            ui.add(
                                egui::Hyperlink::from_label_and_url(
                                    "Contribute this puzzle to Number Loom",
                                    "https://forms.gle/WXxWVsEMqy3NHXmK9",
                                )
                                .open_in_new_tab(true),
                            );
                        } else {
                            ui.label(
                                "If you'd like to contribute your puzzle to Number Loom's \
                                library, please set the license to 'CC BY 4.0'",
                            );
                        }

                        ui.separator();

                        ui.label("Paste a 'WOVEN' string to load:");
                        ui.add(
                            egui::TextEdit::multiline(&mut self.pasted_string)
                                .font(TextStyle::Monospace)
                                .desired_width(730.0),
                        );

                        if ui.button("Load").clicked() {
                            match crate::formats::woven::from_woven(&self.pasted_string) {
                                Ok(doc) => {
                                    new_document = Some(doc);
                                    next_enter_solve_mode = true;
                                }
                                Err(e) => {
                                    self.editor_gui.status.set(StatusMessage::error(format!(
                                        "Error loading WOVEN puzzle: {:?}",
                                        e
                                    )));
                                }
                            }
                        }

                        ui.separator();

                        ui.label("Supported file types:");
                        ui.label("  .png (or other image formats): solution image");
                        ui.label("  .xml/.pbn: the format used by the \"pbnsolve\" solver");
                        ui.label("  .txt: grid of characters");
                        ui.label("  .g: the format used by the Olšák solver");
                        ui.label("  .woven: Number Loom's custom format");
                        ui.label("  .html: printable puzzle");

                        ui.horizontal(|ui| {
                            ui.label("Filename:");
                            ui.add(
                                egui::TextEdit::singleline(&mut self.editor_gui.document.file)
                                    .desired_width(450.0),
                            );
                        });
                        if ui.button("Save").clicked() {
                            let mut document_copy = self.editor_gui.document.clone();

                            let (sender, receiver) = mpsc::channel();
                            self.save_result_receiver = receiver;

                            spawn_async(async move {
                                let handle = rfd::AsyncFileDialog::new()
                                    .add_filter(
                                        "all recognized formats",
                                        &["png", "gif", "bmp", "xml", "pbn", "txt", "g", "html"],
                                    )
                                    .add_filter("image", &["png", "gif", "bmp"])
                                    .add_filter("PBN", &["xml", "pbn"])
                                    .add_filter("chargrid", &["txt"])
                                    .add_filter("Olšák", &["g"])
                                    .add_filter("woven", &["woven"])
                                    .add_filter("HTML (for printing)", &["html"])
                                    .set_file_name(document_copy.file.clone())
                                    .save_file()
                                    .await;

                                if let Some(handle) = handle {
                                    let result = async {
                                        let bytes = to_bytes(
                                            &mut document_copy,
                                            Some(handle.file_name()),
                                            None,
                                        )?;
                                        handle.write(&bytes).await?;
                                        Ok(())
                                    }
                                    .await;
                                    sender.send(result).unwrap();
                                }
                            });
                        }

                        if let Ok(Err(e)) = self.save_result_receiver.try_recv() {
                            self.editor_gui
                                .status
                                .set(StatusMessage::error(format!("Error saving file: {:?}", e)));
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
                self.show_save_share_window = false;
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

#[derive(PartialEq, Eq)]
enum NewPuzzleShape {
    Square,
    Triangular,
}

struct NewPuzzleDialog {
    shape: NewPuzzleShape,
    clue_style: crate::puzzle::ClueStyle,
    x_size: usize,
    y_size: usize,
    /// Hexagon side length, used only when `shape` is `Triangular`. Doesn't cover every possible
    /// triddler outline (see `Outline`) — just a reasonable default shape to start editing from.
    tri_side: i32,
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

        egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
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

        egui::CentralPanel::default().show(ctx, |ui| {
            self.main_ui(ctx, ui);
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

        *progress.borrow_mut() = if self.progress > 0.0 && self.progress < 1.0 {
            Some(self.progress)
        } else {
            None
        };

        if ui
            .add_enabled(self.report.is_some(), egui::Button::new("Clear"))
            .clicked()
        {
            self.report = None;
        }
    }
}

#[cfg(test)]
mod lasso_tests {
    use super::*;
    use crate::geometry::{Geometry, Outline, Tri};
    use crate::puzzle::{ClueStyle, Solution};

    fn square(w: usize, h: usize) -> DynSolution {
        DynSolution::Square(Solution::blank_bw(w, h))
    }

    fn selected(mask: &[bool]) -> Vec<u32> {
        mask.iter()
            .enumerate()
            .filter(|(_, m)| **m)
            .map(|(i, _)| i as u32)
            .collect()
    }

    /// A loop traced around the middle 3×3 of a 5×5 grid selects exactly those nine cells: the
    /// eight the path runs through, plus the one in the centre that it only encloses.
    #[test]
    fn a_loop_selects_what_it_traces_and_what_it_encloses() {
        let picture = square(5, 5);
        // Through the centres of the ring cells, so "touched" is unambiguous.
        let path = vec![
            Point::new(1.5, 1.5),
            Point::new(3.5, 1.5),
            Point::new(3.5, 3.5),
            Point::new(1.5, 3.5),
        ];
        let mask = cells_in_lasso(&picture, &path);
        let want: Vec<u32> = (1..=3)
            .flat_map(|y| (1..=3).map(move |x| y * 5 + x))
            .collect();
        assert_eq!(selected(&mask), want);
    }

    /// The ends are joined by a straight line, so a path that stops short still closes — the
    /// player never has to land exactly back where they started.
    #[test]
    fn an_open_path_is_closed_across_the_gap() {
        let picture = square(5, 5);
        // Three sides of the same box: the fourth is supplied by the closing segment.
        let open = vec![
            Point::new(1.5, 1.5),
            Point::new(3.5, 1.5),
            Point::new(3.5, 3.5),
            Point::new(1.5, 3.5),
            Point::new(1.5, 2.5),
        ];
        let closed = vec![
            Point::new(1.5, 1.5),
            Point::new(3.5, 1.5),
            Point::new(3.5, 3.5),
            Point::new(1.5, 3.5),
        ];
        assert_eq!(
            selected(&cells_in_lasso(&picture, &open)),
            selected(&cells_in_lasso(&picture, &closed))
        );
    }

    /// A fast drag reports few points, but the path between them still counts as touched —
    /// otherwise a quick diagonal flick would select a dotted line of cells.
    #[test]
    fn a_sparse_path_still_touches_every_cell_it_crosses() {
        let picture = square(5, 1);
        let path = vec![Point::new(0.5, 0.5), Point::new(4.5, 0.5)];
        assert_eq!(
            selected(&cells_in_lasso(&picture, &path)),
            vec![0, 1, 2, 3, 4]
        );
    }

    /// The outline is the boundary and nothing else: a single square cell contributes its four
    /// edges, and two side-by-side cells contribute six, not eight — the shared edge cancels.
    #[test]
    fn the_outline_drops_shared_edges() {
        let picture = square(3, 3);
        assert_eq!(selection_outline(&picture, &[0]).len(), 4);
        assert_eq!(selection_outline(&picture, &[0, 1]).len(), 6);
        // A 2×2 block: eight boundary edges, with the four interior ones cancelled.
        assert_eq!(selection_outline(&picture, &[0, 1, 3, 4]).len(), 8);
    }

    /// The dense cell index of `(x, y)` in the 6×6 grid the move tests use.
    fn at(x: usize, y: usize) -> usize {
        y * 6 + x
    }

    /// A canvas over a blank 6×6 with a 2×2 block of `Color(1)` at (1,1).
    fn canvas_with_a_block() -> CanvasGui {
        let mut sol = Solution::blank_bw(6, 6);
        for (x, y) in [(1, 1), (2, 1), (1, 2), (2, 2)] {
            sol.cells[at(x, y)] = Color(1);
        }
        let mut gui = NonogramGui::new(Document::from_solution(
            DynSolution::Square(sol),
            "test".to_string(),
        ))
        .editor_gui;
        gui.current_tool = Tool::Lasso;
        gui
    }

    fn press(gui: &mut CanvasGui, p: Point) {
        gui.lasso_input(
            LassoPointer {
                pressed: true,
                down: true,
                ..Default::default()
            },
            p,
        );
    }

    fn drag(gui: &mut CanvasGui, p: Point) {
        gui.lasso_input(
            LassoPointer {
                down: true,
                ..Default::default()
            },
            p,
        );
    }

    fn release(gui: &mut CanvasGui, p: Point) {
        gui.lasso_input(
            LassoPointer {
                released: true,
                ..Default::default()
            },
            p,
        );
    }

    /// Lasso the block, drag it two cells right and one down, then switch tools. The block should
    /// be at its new home, and every cell it came from should be background.
    #[test]
    fn a_dragged_selection_moves_and_leaves_background_behind() {
        let mut gui = canvas_with_a_block();

        press(&mut gui, Point::new(0.5, 0.5));
        for p in [
            Point::new(3.5, 0.5),
            Point::new(3.5, 3.5),
            Point::new(0.5, 3.5),
        ] {
            drag(&mut gui, p);
        }
        release(&mut gui, Point::new(0.5, 3.5));

        // Grab inside the selection and drag it.
        press(&mut gui, Point::new(1.5, 1.5));
        drag(&mut gui, Point::new(3.5, 2.5));
        release(&mut gui, Point::new(3.5, 2.5));

        // Still floating: the source is already background, the destination not yet stamped.
        let cells = gui.document.try_solution().unwrap().cells();
        assert!(cells.iter().all(|c| *c == BACKGROUND), "source not cleared");

        gui.clear_selection();

        let cells = gui.document.try_solution().unwrap().cells();
        let lit: Vec<usize> = cells
            .iter()
            .enumerate()
            .filter(|(_, c)| **c == Color(1))
            .map(|(i, _)| i)
            .collect();
        // The block moved by (+2, +1): (1,1)..(2,2) became (3,2)..(4,3).
        let want: Vec<usize> = [(3, 2), (4, 2), (3, 3), (4, 3)]
            .iter()
            .map(|(x, y)| at(*x, *y))
            .collect();
        assert_eq!(lit, want);
    }

    /// Only non-background cells are stamped, so a moved shape composites over what's already
    /// there rather than carrying a background-coloured box along with it.
    #[test]
    fn a_move_does_not_erase_at_the_destination() {
        let mut gui = canvas_with_a_block();
        // A lone cell at (4,1), in the path of the incoming selection's background corner.
        let target = at(4, 1);
        gui.document.solution_mut().cells_mut()[target] = Color(1);

        // Lasso a 3×3 region covering the block plus background at its right edge.
        press(&mut gui, Point::new(0.5, 0.5));
        for p in [
            Point::new(3.5, 0.5),
            Point::new(3.5, 3.5),
            Point::new(0.5, 3.5),
        ] {
            drag(&mut gui, p);
        }
        release(&mut gui, Point::new(0.5, 3.5));

        // Shift right by two: the selection's background cells now overlap (4,1).
        press(&mut gui, Point::new(1.5, 1.5));
        drag(&mut gui, Point::new(3.5, 1.5));
        release(&mut gui, Point::new(3.5, 1.5));
        gui.clear_selection();

        let cells = gui.document.try_solution().unwrap().cells();
        assert_eq!(cells[target], Color(1), "a background cell overwrote art");
    }

    /// Flattening is one undoable step on top of the lift, so two undos put everything back.
    #[test]
    fn a_move_can_be_undone() {
        let mut gui = canvas_with_a_block();
        let original = gui.document.try_solution().unwrap().cells().to_vec();

        press(&mut gui, Point::new(0.5, 0.5));
        for p in [
            Point::new(3.5, 0.5),
            Point::new(3.5, 3.5),
            Point::new(0.5, 3.5),
        ] {
            drag(&mut gui, p);
        }
        release(&mut gui, Point::new(0.5, 3.5));

        press(&mut gui, Point::new(1.5, 1.5));
        drag(&mut gui, Point::new(3.5, 2.5));
        release(&mut gui, Point::new(3.5, 2.5));
        gui.clear_selection();

        gui.un_or_re_do(true); // The flatten.
        gui.un_or_re_do(true); // The lift.
        assert_eq!(gui.document.try_solution().unwrap().cells(), original);
    }

    /// Dragging off the edge and back loses nothing: the mask keeps the whole selection, and only
    /// flattening makes the clipping permanent.
    #[test]
    fn dragging_past_the_edge_and_back_restores_everything() {
        let mut gui = canvas_with_a_block();
        let original = gui.document.try_solution().unwrap().cells().to_vec();

        press(&mut gui, Point::new(0.5, 0.5));
        for p in [
            Point::new(3.5, 0.5),
            Point::new(3.5, 3.5),
            Point::new(0.5, 3.5),
        ] {
            drag(&mut gui, p);
        }
        release(&mut gui, Point::new(0.5, 3.5));

        press(&mut gui, Point::new(1.5, 1.5));
        drag(&mut gui, Point::new(-8.5, 1.5)); // Well off the left edge.
        drag(&mut gui, Point::new(1.5, 1.5)); // ...and back to where it started.
        release(&mut gui, Point::new(1.5, 1.5));
        gui.clear_selection();

        assert_eq!(gui.document.try_solution().unwrap().cells(), original);
    }

    /// Triangles too: a ▲ and the ▼ beside it share an edge, so their outline is a rhombus with
    /// four sides rather than two separate triangles with six.
    #[test]
    fn the_outline_works_for_triangles() {
        let sol: Solution<Tri> = Solution::new(
            ClueStyle::Nono,
            HashMap::from([(BACKGROUND, ColorInfo::default_bg())]),
            Geometry::new(Outline::hexagon(2)),
            vec![BACKGROUND; Geometry::<Tri>::new(Outline::hexagon(2)).cell_count()],
        );
        let picture = DynSolution::Tri(sol);
        assert_eq!(selection_outline(&picture, &[0]).len(), 3);
        // Cells 0 and 1 are adjacent within the top row of the hexagon.
        assert_eq!(selection_outline(&picture, &[0, 1]).len(), 4);
    }
}
