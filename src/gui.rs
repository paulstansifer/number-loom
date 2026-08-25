mod resize;
mod toolbar;

pub use toolbar::LibraryStatus;
use toolbar::NewPuzzleDialog;

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

use crate::{
    export::to_bytes,
    grid_solve::{self, DisambigResult, disambig_candidates},
    gui_solver::{RenderStyle, SolveGui},
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

/// Find a color that's definitely safe, at least.
pub fn default_color(palette: &Palette) -> Color {
    if palette.contains_key(&Color(1)) {
        Color(1)
    } else {
        BACKGROUND
    }
}

/// The number-key shortcut for the `index`th palette entry, in the order the palette editor
/// shows them, along with the character to name it in a tooltip.
fn palette_shortcut(index: usize) -> Option<(egui::Key, char)> {
    use egui::Key::*;
    const KEYS: [(egui::Key, char); 10] = [
        (Num1, '1'),
        (Num2, '2'),
        (Num3, '3'),
        (Num4, '4'),
        (Num5, '5'),
        (Num6, '6'),
        (Num7, '7'),
        (Num8, '8'),
        (Num9, '9'),
        (Num0, '0'),
    ];
    KEYS.get(index).copied()
}

/// The icon, bare-key shortcut, and tooltip key-name for each tool.
fn tool_appearance(tool: Tool) -> (&'static str, egui::Key, char) {
    match tool {
        Tool::Pencil => (icons::ICON_BRUSH, egui::Key::P, 'P'),
        Tool::LineAlongLane => (icons::ICON_LINE_START, egui::Key::L, 'L'),
        Tool::FloodFill => (icons::ICON_FORMAT_COLOR_FILL, egui::Key::F, 'F'),
        // `L` is spoken for by the line tool, so the lasso gets "select" instead.
        Tool::Lasso => (icons::ICON_LASSO_SELECT, egui::Key::S, 'S'),
    }
}

/// One entry in the tool row: a toggle button that its key also reaches. `typing` suppresses the
/// key while a `TextEdit` has the keyboard.
fn tool_button(
    ui: &mut egui::Ui,
    current_tool: &mut Tool,
    tool: Tool,
    typing: bool,
    description: &str,
) {
    let (icon, key, ch) = tool_appearance(tool);
    ui.selectable_value(current_tool, tool, egui::RichText::new(icon).size(24.0))
        .on_hover_text(format!("{description} (press {ch})"));
    if !typing && ui.input(|i| i.key_pressed(key)) {
        *current_tool = tool;
    }
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
                    // and a floating layer belongs to the picture it was lifted from.
                    self.selection = None;
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

        ui.separator();

        self.palette_editor(ui, palette_read_only);
    }

    fn tool_selector(&mut self, ui: &mut egui::Ui, editing: bool) {
        let was = self.current_tool;

        // Same story as in `common_sidebar_items`: no modifiers here either.
        let typing = ui.ctx().wants_keyboard_input();

        centered_row(ui, "tools", |ui| {
            tool_button(ui, &mut self.current_tool, Tool::Pencil, typing, "Pencil");
            tool_button(
                ui,
                &mut self.current_tool,
                Tool::LineAlongLane,
                typing,
                "Line along a row, column or diagonal",
            );
            // Flood fill and the lasso are editor-only, so their keys are dead in the solver
            // rather than silently switching to a tool with no button.
            if editing {
                tool_button(
                    ui,
                    &mut self.current_tool,
                    Tool::FloodFill,
                    typing,
                    "Flood Fill",
                );

                tool_button(
                    ui,
                    &mut self.current_tool,
                    Tool::Lasso,
                    typing,
                    "Lasso select: draw a loop, then drag to move what's inside",
                );
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
    /// re-base the selection there. It remains selected, though.
    ///
    /// Only non-background cells are stamped. Cells dragged off the grid are dropped.
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

/// The contiguous block of one color under the pointer, as the clue gutters need it: which lane
/// it runs along in each clue family, and how long it is there. The same figure the sidebar
/// rosette adds up — its two arms, plus the hovered cell itself.
#[derive(Clone, PartialEq, Debug)]
pub struct HoverBlocks {
    /// `(lane, length)` per clue family, as `DynSolution::blocks_at_cell` reports it.
    pub by_family: Vec<(usize, usize)>,
    /// The hovered cell's color: what the numbers are drawn in.
    pub rgb: (u8, u8, u8),
}

impl HoverBlocks {
    /// How long the hovered block is along `lane`, if it runs along that lane at all.
    pub fn on_lane(&self, lane: usize) -> Option<usize> {
        self.by_family
            .iter()
            .find(|(l, _)| *l == lane)
            .map(|(_, len)| *len)
    }
}

/// What a canvas needs in order to draw clue gutters around the picture. Only the solve view
/// supplies this; the editor draws the picture alone.
pub struct ClueOverlay<'a> {
    pub puzzle: &'a crate::puzzle::DynPuzzle,
    /// One `Vec<LineStatus>` per clue family, in family order.
    pub analysis: Option<&'a Vec<Vec<crate::grid_solve::LineStatus>>>,
    pub is_stale: bool,
    /// The hovered cell's block lengths, shown in place of the analysis marks on its own lanes.
    pub hover: Option<HoverBlocks>,
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

        // The picture's own area, with any clue gutters excluded: this is somewhere a click
        // reliably lands on a cell.
        self.picture_rect = Some(to_screen.transform_rect(Rect::from_min_size(
            Pos2::ZERO,
            Vec2::new(extent.x, extent.y),
        )));

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
        } else if hovered_cell.is_some() {
            // There's no brush or paint-bucket in the standard cursor set, so the best these can
            // do is say how precise the tool is: the two that paint a cell the pointer is exactly
            // on get a crosshair, and flood fill — which acts on a whole region — gets the
            // blockier `Cell` instead, just so it doesn't look identical to them.
            ui.ctx().set_cursor_icon(match self.current_tool {
                Tool::Pencil | Tool::LineAlongLane => egui::CursorIcon::Crosshair,
                Tool::FloodFill => egui::CursorIcon::Cell,
                Tool::Lasso => unreachable!("handled above"),
            });
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
                // Paranoia, since it would cause a crash.
                debug_assert!(
                    picture.palette().contains_key(&paint_color),
                    "painting with {paint_color:?}, which is not in the palette"
                );
                let paint_color = if picture.palette().contains_key(&paint_color) {
                    paint_color
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
            || disambiguator.is_some_and(|d| d.progress > 0.0 && d.progress < 1.0);

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
                        solved_mask.is_none_or(|sm| sm.1[index]) || overlays_suppress_unsolved;
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
                // Background cells don't move.
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

                    // The indicator strip between the clues and the grid: the hovered block's
                    // length on the three lanes it runs along, and the analysis mark (which the
                    // number deliberately covers up) everywhere else.
                    let at = to_screen
                        * Pos2::new(
                            g.anchor.x + g.outward.x * (crate::layout::CLUE_PAD / 2.0),
                            g.anchor.y + g.outward.y * (crate::layout::CLUE_PAD / 2.0),
                        );
                    let hovered = overlay
                        .hover
                        .as_ref()
                        .and_then(|h| Some((h.on_lane(g.lane)?, h.rgb)));
                    match hovered {
                        Some((len, rgb)) => crate::gui_solver::draw_bare_number(
                            ui,
                            &painter,
                            at,
                            &len.to_string(),
                            scale,
                            rgb,
                        ),
                        None => {
                            if let Some(analysis) = overlay.analysis {
                                let family = lane_families[g.lane];
                                let index = g.lane - family_starts[family];
                                if let Some(status) =
                                    analysis.get(family).and_then(|f| f.get(index))
                                {
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
                // The loop as it's being drawn (open)
                let points: Vec<Pos2> = path
                    .iter()
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

        let mut changes = HashMap::new();
        if start == end {
            changes.insert(end, self.drag_start_color);
            return changes;
        }

        // Cell *centres*, not raw origins: a triangle's centroid sits off-corner and at a
        // different offset for ▲ than ▼, so mixing origins would misjudge lane direction
        // whenever a lane's cells alternate orientation.
        let center = |cell: u32| picture.cell_shape(cell).center(picture.cell_origin(cell));

        let start_center = center(start);
        let end_center = center(end);
        let drag =
            crate::layout::Vec2::new(end_center.x - start_center.x, end_center.y - start_center.y);
        let drag_len = (drag.x * drag.x + drag.y * drag.y).sqrt();

        // A lane's cells zigzag between ▲ and ▼ centroids on a triangular grid, so the step to
        // an immediate neighbor is not representative of the lane's direction — e.g. from a ▲,
        // the very next step is purely vertical even on a "/" lane. Use the span from the lane's
        // first cell to its last instead, which averages the zigzag out into the lane's true
        // on-screen direction, and gives a stable average per-cell spacing along it.
        let mut best: Option<(usize, f32)> = None; // (lane, |cos angle| to drag)
        for membership in lanes.memberships(start) {
            let lane = lanes.lane(membership.lane as usize);
            if lane.cells.len() < 2 {
                continue; // No direction to compare against.
            }
            let first = center(lane.cells[0]);
            let last = center(*lane.cells.last().unwrap());
            let span = crate::layout::Vec2::new(last.x - first.x, last.y - first.y);
            let span_len = (span.x * span.x + span.y * span.y).sqrt();
            // Angle between the lane's direction and the drag, ignoring which way along the
            // lane it points, so dragging toward either end still snaps to that lane.
            let cos_angle = ((span.x * drag.x + span.y * drag.y) / (span_len * drag_len)).abs();
            if best.is_none_or(|(_, best_cos)| cos_angle > best_cos) {
                best = Some((membership.lane as usize, cos_angle));
            }
        }

        match best {
            Some((lane_idx, _)) => {
                let lane = lanes.lane(lane_idx);
                let from_pos = lanes
                    .memberships(start)
                    .iter()
                    .find(|m| m.lane as usize == lane_idx)
                    .unwrap()
                    .position as usize;

                let first = center(lane.cells[0]);
                let last = center(*lane.cells.last().unwrap());
                let span = crate::layout::Vec2::new(last.x - first.x, last.y - first.y);
                let span_len = (span.x * span.x + span.y * span.y).sqrt();
                let avg_spacing = span_len / (lane.cells.len() - 1) as f32;

                // Distance travelled along the lane, in cell steps, found by projecting the
                // drag onto the lane's own (first-to-last) direction.
                let signed_distance = (drag.x * span.x + drag.y * span.y) / span_len;
                let delta = (signed_distance / avg_spacing).round() as isize;

                let to_pos =
                    (from_pos as isize + delta).clamp(0, lane.cells.len() as isize - 1) as usize;
                let (from, to) = (from_pos.min(to_pos), from_pos.max(to_pos));
                for cell in &lane.cells[from..=to] {
                    changes.insert(*cell, self.drag_start_color);
                }
            }
            // `start` sits in no lane with a usable direction; just paint the endpoint.
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

        // Same story as in `common_sidebar_items`: these shortcuts have no modifier, so they
        // have to stand down by hand while a `TextEdit` has the keyboard.
        let typing = ui.ctx().wants_keyboard_input();

        use itertools::Itertools;

        for (index, (color, color_info)) in self
            .document
            .solution_mut()
            .palette_mut()
            .iter_mut()
            .sorted_by_key(|(color, _)| *color)
            // TODO: actually paint a palette entry for unsolved,
            // in case the user doesn't have a middle button.
            .filter(|(color, _)| !(**color == UNSOLVED && read_only))
            .enumerate()
        {
            let shortcut = palette_shortcut(index);
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
                let hover_text = match shortcut {
                    Some((_, ch)) => format!("Paint with {} (press {})", color_info.name, ch),
                    None => format!("Paint with {}", color_info.name),
                };
                if ui
                    .add(egui::Button::new(color_text))
                    .on_hover_text(hover_text)
                    .clicked()
                    || (!typing
                        && shortcut.is_some_and(|(key, _)| ui.input(|i| i.key_pressed(key))))
                {
                    picked_color = *color;
                };

                if !read_only {
                    let mut edited_color = [r as f32 / 256.0, g as f32 / 256.0, b as f32 / 256.0];

                    let edit = ui.color_edit_button_rgb(&mut edited_color);
                    // `egui` only allows rectangular-swatch-of-current-color as the palette marker,
                    // which doesn't look good in this case. (In fact, the color is also somewhat wrong)
                    // HACK: draw a pencil icon over it.
                    let visuals = *ui.style().interact(&edit);
                    let painter = ui.painter();
                    painter.rect(
                        edit.rect,
                        visuals.corner_radius,
                        visuals.bg_fill,
                        visuals.bg_stroke,
                        egui::StrokeKind::Inside,
                    );
                    painter.text(
                        edit.rect.center(),
                        egui::Align2::CENTER_CENTER,
                        icons::ICON_EDIT,
                        egui::FontId::proportional(edit.rect.height() * 0.7),
                        visuals.fg_stroke.color,
                    );
                    if edit.on_hover_text("Edit this color").changed() {
                        // TODO: this should probably also be undoable
                        picked_color = *color;
                        color_info.rgb = (
                            (edited_color[0] * 256.0) as u8,
                            (edited_color[1] * 256.0) as u8,
                            (edited_color[2] * 256.0) as u8,
                        );
                    }
                    if *color != BACKGROUND && ui.button(icons::ICON_DELETE).clicked() {
                        removed_color = Some(*color);
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
                    document: Box::new(new_document),
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
                    document: Box::new(new_document),
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
/// Found by cancellation: push every selected cell's edges into a table, and an edge shared by
/// two selected cells lands there twice. What's left having landed once is exactly the boundary.
/// Works for squares and triangles.
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

/// Stitch a boundary's unordered edges into closed loops, each a list of points ending back where
/// it started. A cell's edges are wound consistently, and that winding survives cancellation, so
/// each vertex is the tail of exactly one surviving edge: following tail-to-head therefore always
/// closes a loop.
///
/// Loops matter (rather than the raw edge list) so the marching ants can be drawn as one dashed
/// path per loop: dashing a whole path keeps the dash phase continuous across corners, where
/// dashing each edge in isolation would restart the pattern at every corner.
fn outline_loops(outline: &[(Point, Point)]) -> Vec<Vec<Point>> {
    type Vertex = (i32, i32);
    let key =
        |p: Point| -> Vertex { ((p.x * 4096.0).round() as i32, (p.y * 4096.0).round() as i32) };

    let mut next: HashMap<Vertex, (Point, Point)> = HashMap::new();
    for &(a, b) in outline {
        next.insert(key(a), (a, b));
    }

    let mut loops = Vec::new();
    let mut visited: std::collections::HashSet<Vertex> = std::collections::HashSet::new();
    for &(start, _) in outline {
        let start_key = key(start);
        if !visited.insert(start_key) {
            continue;
        }
        let mut loop_points = vec![start];
        let mut cur = start_key;
        while let Some(&(_, b)) = next.get(&cur) {
            loop_points.push(b);
            cur = key(b);
            if cur == start_key || !visited.insert(cur) {
                break;
            }
        }
        loops.push(loop_points);
    }
    loops
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
    let loops = outline_loops(outline);
    let mut shapes = Vec::with_capacity(loops.len() * 2);
    // `dashed_line_with_offset` walks the offset forward from each path's start and assumes it's
    // non-negative; a negative one makes it extrapolate the first dash backward past the start
    // point, flashing a stray segment there. `%` alone can return negative, so use `rem_euclid`.
    let offset = (-(elapsed * ANT_SPEED)).rem_euclid(ANT_DASH * 2.0);

    for loop_points in loops {
        let points: Vec<Pos2> = loop_points
            .iter()
            .map(|p| to_screen * Pos2::new(p.x, p.y))
            .collect();
        shapes.push(Shape::line(
            points.clone(),
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

    // A `Corner` color is a half-square used by trianogram clues, and appears only on a square grid. It's a different thing
    // from a triangular *cell*.
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

        self.solve_gui = Some(crate::gui_solver::SolveGui::new(
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

    /// Only non-background cells are stamped.
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

#[cfg(test)]
mod line_tool_tests {
    use super::*;
    use crate::puzzle::Solution;

    fn at(x: usize, y: usize) -> u32 {
        (y * 6 + x) as u32
    }

    fn canvas() -> CanvasGui {
        let sol = Solution::blank_bw(6, 6);
        let mut gui = NonogramGui::new(Document::from_solution(
            DynSolution::Square(sol),
            "test".to_string(),
        ))
        .editor_gui;
        gui.current_tool = Tool::LineAlongLane;
        gui.drag_start_color = Color(1);
        gui
    }

    /// Dragging mostly-horizontally, but not to a cell that shares the start's row, still snaps
    /// to the row: the nearest lane by angle, not an exact row/column match.
    #[test]
    fn a_near_horizontal_drag_snaps_to_the_row() {
        let mut gui = canvas();
        // From (1,1), drag toward (4,2): mostly rightward with a little drop, so the row
        // (angle 0) is closer than the column (angle 90) to the drag direction.
        let changes = gui.line_between(at(1, 1), at(4, 2));

        let mut got: Vec<u32> = changes.keys().copied().collect();
        got.sort();
        assert_eq!(got, vec![at(1, 1), at(2, 1), at(3, 1), at(4, 1)]);
        assert!(changes.values().all(|c| *c == Color(1)));
    }

    /// Symmetric case: a near-vertical drag snaps to the column instead.
    #[test]
    fn a_near_vertical_drag_snaps_to_the_column() {
        let mut gui = canvas();
        // From (1,1), drag toward (2,4): mostly downward with a little sideways drift.
        let changes = gui.line_between(at(1, 1), at(2, 4));

        let mut got: Vec<u32> = changes.keys().copied().collect();
        got.sort();
        assert_eq!(got, vec![at(1, 1), at(1, 2), at(1, 3), at(1, 4)]);
    }

    /// A drag that goes backward along the lane (toward its start) still snaps and paints the
    /// span behind the origin, not just the endpoint.
    #[test]
    fn dragging_backward_along_a_lane_still_draws_the_span() {
        let mut gui = canvas();
        let changes = gui.line_between(at(4, 2), at(1, 3));

        let mut got: Vec<u32> = changes.keys().copied().collect();
        got.sort();
        assert_eq!(got, vec![at(1, 2), at(2, 2), at(3, 2), at(4, 2)]);
    }

    /// A triangular grid's `/` and `\` lanes zigzag between ▲ and ▼ cell centroids, so a lane's
    /// direction can't be judged from a single neighboring cell: from a ▲, the very next cell
    /// along a "/" lane sits directly *below* it (a purely vertical step), which used to fool the
    /// snapping into thinking that lane wasn't diagonal at all. Regardless of which orientation
    /// a lane starts (or ends) on, dragging along its full length should paint the whole thing.
    #[test]
    fn diagonal_lanes_are_reachable_from_either_triangle_orientation() {
        use crate::geometry::{Geometry, Outline, Tri};
        use crate::puzzle::ClueStyle;

        let geometry = Geometry::<Tri>::new(Outline::hexagon(2));
        let sol: Solution<Tri> = Solution::new(
            ClueStyle::Nono,
            HashMap::from([(BACKGROUND, ColorInfo::default_bg())]),
            geometry,
            vec![BACKGROUND; Geometry::<Tri>::new(Outline::hexagon(2)).cell_count()],
        );
        let lane_map = sol.geometry.lane_map().clone();
        let mut gui = NonogramGui::new(Document::from_solution(
            DynSolution::Tri(sol),
            "test".to_string(),
        ))
        .editor_gui;
        gui.current_tool = Tool::LineAlongLane;
        gui.drag_start_color = Color(1);

        // Every "/" and "\" lane (families 1 and 2) with more than one cell: dragging end to end
        // should paint every cell in it, no matter which orientation each end happens to be.
        for family in [1usize, 2usize] {
            for lane_idx in lane_map.family(family) {
                let lane = lane_map.lane(lane_idx);
                if lane.cells.len() < 2 {
                    continue;
                }
                let first = lane.cells[0];
                let last = *lane.cells.last().unwrap();

                let mut got: Vec<u32> = gui.line_between(first, last).keys().copied().collect();
                got.sort();
                let mut want = lane.cells.clone();
                want.sort();
                assert_eq!(
                    got,
                    want,
                    "family {family} lane {lane_idx} (len {}) didn't paint end to end",
                    lane.cells.len()
                );
            }
        }
    }
}

#[cfg(test)]
mod palette_tests {
    use super::*;
    use crate::puzzle::Solution;

    fn doc(sol: Solution<crate::geometry::Square>) -> Document {
        Document::from_solution(DynSolution::Square(sol), "test".to_string())
    }

    #[test]
    fn swapping_the_document_cant_leave_the_color_dangling() {
        let mut fancy = Solution::blank_bw(3, 3);
        fancy
            .palette
            .insert(Color(3), ColorInfo::default_fg(Color(3)));

        let mut gui = NonogramGui::new(doc(fancy)).editor_gui;
        gui.current_color = Color(3);
        gui.drag_start_color = Color(3);

        // The black-and-white palette has no `Color(3)`.
        gui.perform(
            Action::ReplaceDocument {
                document: Box::new(doc(Solution::blank_bw(3, 3))),
            },
            ActionMood::Normal,
        );

        let palette = gui.document.try_solution().unwrap().palette();
        assert!(palette.contains_key(&gui.current_color));
        assert!(palette.contains_key(&gui.drag_start_color));
    }

    #[test]
    fn a_color_the_new_palette_still_has_is_left_alone() {
        let mut gui = NonogramGui::new(doc(Solution::blank_bw(3, 3))).editor_gui;
        gui.current_color = BACKGROUND;
        gui.drag_start_color = BACKGROUND;

        gui.perform(
            Action::ReplaceDocument {
                document: Box::new(doc(Solution::blank_bw(4, 4))),
            },
            ActionMood::Normal,
        );

        assert_eq!(gui.current_color, BACKGROUND);
        assert_eq!(gui.drag_start_color, BACKGROUND);
    }
}
