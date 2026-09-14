mod annotate;
mod auto_button;
mod canvas;
pub mod gallery;
mod outline_text;
mod palette;
mod resize;
mod selection;
pub mod solver;
mod toolbar;
mod tools;
mod triano;
mod triddler;

pub use annotate::{AnnotateDrag, Annotation};
pub use auto_button::{AutoButton, auto_button};
pub use canvas::{ClueId, ClueOverlay, HoverBlocks};
pub use palette::default_color;
pub use selection::{Selection, cells_in_lasso};
pub use toolbar::LibraryStatus;
use toolbar::NewPuzzleDialog;
pub use tools::Tool;

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::mpsc,
    time::Duration,
};

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
    import,
    // The abstract-units point, distinct from egui's `Pos2`: everything the lasso does is in
    // grid space, and only the painter converts.
    layout::Point,
    puzzle::{
        BACKGROUND, ClueStyle, Color, ColorInfo, Document, DynSolution, Palette, PuzzleDynOps,
        Solution, UNSOLVED,
    },
    solve::bt_solve,
    solve::grid_solve::{self, DisambigResult, SolveOptions, disambig_candidates},
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
    /// Whatever was current before `current_tool`, so that a tool's own key, pressed again, goes
    /// back to it. Starts out equal to `current_tool`, which makes that first press a no-op.
    pub previous_tool: Tool,
    pub line_tool_state: Option<u32>,
    /// The lasso tool's selection, if any. Outlives switching tools only long enough to be
    /// flattened; see `flatten_selection`.
    pub selection: Option<Selection>,
    /// The annotate tool's scratch marks. Deliberately outside the undo system: none of these is
    /// an `Action`, and none of them bumps `version`.
    pub annotations: Vec<Annotation>,
    /// The clues the solver's gutters have been checked off by hand, keyed by `(lane, index
    /// within the lane)`. Undoable (`Action::ToggleClue`), but no part of the picture, so it
    /// doesn't bump `version` and the solve replay steps straight past it.
    pub checked_clues: HashSet<ClueId>,
    /// The annotation being dragged out right now, if any.
    pub annotate_drag: Option<AnnotateDrag>,
    /// Whether this canvas is solving a puzzle rather than editing one. That decides which tools
    /// it offers (annotate is solver-only, the way flood fill and the lasso are editor-only),
    /// whether the palette can be edited, and whether a cell has an "unknown" state to go back
    /// to when a click undoes itself.
    pub solving: bool,
    /// Whether the view has somewhere to pan to, in which case a middle-drag pans it and so must
    /// not also reach a tool. `main_ui` sets this from the scroll area's own measurements.
    pub middle_pans: bool,
    /// Indexed by dense cell index, like `Solution::cells`.
    pub solved_mask: Staleable<(String, Vec<bool>)>,
    pub disambiguator: Staleable<Disambiguator>,
    pub backtrack_solver: Staleable<BacktrackSolver>,
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
    /// The `editor_gui.version` `solve_report` was computed for, if any — independent of
    /// `editor_gui.solved_mask`'s own freshness, which the backtracking solve also writes to
    /// (see `BacktrackSolver::widget`). Without this, `solved_mask.get_or_refresh` would see
    /// backtracking's fresher write and skip re-running line logic, leaving `solve_report`
    /// showing backtracking's text under the `Solve` button.
    solve_report_version: Option<Version>,
    pub solve_gui: Option<SolveGui>,
    show_save_share_window: bool,
    share_string: String,
    pasted_string: String,
    quality_warnings: Vec<String>,
    /// Whether the picture is too big for the space it's shown in, and so has somewhere to pan
    /// to. Measured from the scroll area as it was drawn last frame.
    pannable: bool,
    /// Whether a middle-drag pan is in flight right now.
    panning: bool,
    /// Wheel motion that hasn't yet added up to a whole step through the palette.
    wheel_remainder: f32,
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
    /// Check a clue off in the solver's gutters, or un-check it. Its own inverse, so undo and
    /// redo are both just the same toggle again.
    ToggleClue {
        clue: ClueId,
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
            Action::ToggleClue { clue } => Action::ToggleClue { clue: *clue },
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
                    // and check-offs name lanes, which the new picture may not have at all.
                    self.selection = None;
                    self.annotations.clear();
                    self.annotate_drag = None;
                    self.checked_clues.clear();
                    // The new palette may not have the color the old one did.
                    self.clamp_colors_to_palette();
                } else {
                    self.status
                        .set(StatusMessage::error("That puzzle has no solution"));
                }
            }
            // No `version` bump: the picture is untouched, so nothing that watches it — the line
            // analysis, the solved mask, the replay — has any reason to recompute.
            Action::ToggleClue { clue } => {
                if !self.checked_clues.remove(&clue) {
                    self.checked_clues.insert(clue);
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

    /// The part of the sidebar the editor and the solver share. `self.solving` says which of
    /// the two is asking, and so which tools and palette controls to offer.
    pub fn common_sidebar_items(&mut self, ui: &mut egui::Ui) {
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

        self.tool_selector(ui);

        // Annotations aren't part of the picture, so undo can't take them back — this is the only
        // way to be rid of them, and it only appears when there's something to clear.
        if self.solving && !self.annotations.is_empty() {
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

        self.palette_editor(ui);
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
                previous_tool: Tool::Pencil,
                line_tool_state: None,
                selection: None,
                annotations: vec![],
                annotate_drag: None,
                checked_clues: HashSet::new(),
                solving: false,
                middle_pans: false,
                picture_rect: None,
                solved_mask: Staleable {
                    val: ("".to_string(), solved_mask),
                    version: 0,
                },
                disambiguator: Staleable {
                    val: Disambiguator::new(),
                    version: 0,
                },
                backtrack_solver: Staleable {
                    val: BacktrackSolver::new(),
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
            solve_report_version: None,
            solve_gui: None,
            show_save_share_window: false,
            share_string: "".to_string(),
            pasted_string: "".to_string(),
            quality_warnings: vec![],
            pannable: false,
            panning: false,
            wheel_remainder: 0.0,
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

            self.editor_gui.common_sidebar_items(ui);

            ui.separator();

            match self.editor_gui.document.try_solution().map(|s| s.shape()) {
                Some(crate::geometry::Shape::Triangular(_)) => self.tri_resizer(ui),
                _ => self.resizer(ui),
            }

            ui.separator();
            let solve = auto_button(ui, "Solve", &mut self.auto_solve);
            if solve.auto.changed() {
                let _ = UserSettings::set(consts::EDITOR_AUTO_SOLVE, &self.auto_solve.to_string());
                if !self.auto_solve {
                    // The shading clears itself (it's only drawn while fresh), but the report is
                    // plain text that would otherwise linger after the aid is switched off.
                    self.solve_report.clear();
                    self.solve_report_version = None;
                }
            }
            if (solve.button.clicked() || self.auto_solve)
                // Tracked separately from `solved_mask`'s own freshness: the backtracking solve
                // writes there too (see `BacktrackSolver::widget`).
                && self.solve_report_version != Some(self.editor_gui.version)
            {
                let puzzle = self.editor_gui.document.try_solution().unwrap().to_puzzle();

                let (report, mask) = match puzzle.plain_solve() {
                    Ok(grid_solve::Report {
                        solve_counts,
                        cells_left,
                        solution: _solution,
                        solved_mask,
                    }) => (
                        // Unsolved cells first: that's the number that says whether the puzzle
                        // works. The skim/scrub counts are solver diagnostics.
                        format!("unsolved cells: {cells_left}\n{solve_counts}"),
                        Some(solved_mask),
                    ),
                    // Can't happen, because the puzzle has a solution!
                    Err(e) => (format!("Error: {:?}", e), None),
                };
                self.solve_report = report.clone();
                self.solve_report_version = Some(self.editor_gui.version);
                if let Some(mask) = mask {
                    self.editor_gui
                        .solved_mask
                        .update((report, mask), self.editor_gui.version);
                }
            }

            ui.colored_label(
                if self.solve_report_version == Some(self.editor_gui.version) {
                    Color32::BLACK
                } else {
                    Color32::GRAY
                },
                &self.solve_report,
            );

            ui.separator();

            let picture = self.editor_gui.document.try_solution().unwrap().clone();
            let version = self.editor_gui.version;
            self.editor_gui
                .backtrack_solver
                .get_or_refresh(version, BacktrackSolver::new)
                .widget(
                    &picture,
                    version,
                    &mut self.editor_gui.solved_mask,
                    &self.editor_gui.progress,
                    ui,
                );

            ui.separator();

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
    /// What to actually draw at: `scale` is what the zoom controls set, but triddlers are drawn
    /// `TRIDDLER_ZOOM_STEPS` clicks further in than square puzzles.
    fn render_scale(&self) -> f32 {
        let shape = match &self.solve_gui {
            Some(solve_gui) => Some(solve_gui.clues.shape()),
            None => self.editor_gui.document.try_solution().map(|s| s.shape()),
        };

        if matches!(shape, Some(crate::geometry::Shape::Triangular(_))) {
            self.scale + TRIDDLER_ZOOM_STEPS * ZOOM_STEP
        } else {
            self.scale
        }
    }

    /// The canvas the user is working in right now.
    fn current_canvas(&mut self) -> &mut CanvasGui {
        match &mut self.solve_gui {
            Some(solve_gui) => &mut solve_gui.canvas,
            None => &mut self.editor_gui,
        }
    }

    fn enter_solve_mode(&mut self) {
        self.solve_gui = Some(SolveGui::new(
            self.editor_gui.document.clone(),
            Rc::clone(&self.editor_gui.status),
            Rc::clone(&self.editor_gui.progress),
        ));
    }

    /// Back to the editor, discarding whatever solving progress was on the board.
    fn exit_solve_mode(&mut self) {
        self.solve_gui = None;
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

/// One click of the zoom buttons, in pixels per cell edge.
pub(crate) const ZOOM_STEP: f32 = 2.0;
/// How far the zoom controls can go, in pixels per cell edge.
pub(crate) const MIN_SCALE: f32 = 1.0;
pub(crate) const MAX_SCALE: f32 = 50.0;

/// How far ahead of the zoom the user set a triddler is drawn. A triangular cell has half a
/// square one's area, and its three clue gutters are rhombuses full of slanted numerals, so the
/// same scale buys a good deal less legibility than it does on a square grid.
const TRIDDLER_ZOOM_STEPS: f32 = 3.0;

/// This frame's wheel motion over the canvas, in notches, with the events taken away from the
/// scroll area beneath — over the canvas the wheel steps through the palette instead of
/// scrolling. Positive is a notch rolled away from the user.
///
/// Read from the events rather than from `raw_scroll_delta` so that one notch is one step
/// whatever the platform's scroll speed happens to be set to.
fn take_wheel_notches(ctx: &egui::Context) -> f32 {
    // Trackpads report points instead of lines; converting at egui's own rate makes a swipe
    // that would have scrolled one line's worth count as one notch.
    let points_per_line = ctx.options(|o| o.line_scroll_speed).max(1.0);

    ctx.input_mut(|i| {
        let mut notches = 0.0;
        i.events.retain(|event| match event {
            egui::Event::MouseWheel {
                unit,
                delta,
                modifiers,
            } => {
                // ctrl/cmd-wheel is egui's zoom gesture; that one belongs to `zoom_delta`.
                if modifiers.ctrl || modifiers.mac_cmd || modifiers.command {
                    return true;
                }
                // Whichever axis moved: shift-wheel arrives as horizontal motion on some
                // platforms, and there's only the one palette to step through either way.
                let amount = if delta.y != 0.0 { delta.y } else { delta.x };
                notches += match unit {
                    egui::MouseWheelUnit::Line | egui::MouseWheelUnit::Page => amount,
                    egui::MouseWheelUnit::Point => amount / points_per_line,
                };
                false
            }
            _ => true,
        });
        // egui spreads a mouse notch over the next few frames, so the leftovers have to go as
        // well — and they arrive on frames that carry no event of their own.
        i.raw_scroll_delta = Vec2::ZERO;
        i.smooth_scroll_delta = Vec2::ZERO;
        notches
    })
}

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
        // egui leaves a button outline-less until it's hovered, which makes a button hard to
        // tell from a label until you go looking for it — and impossible to tell from a second
        // button parked right behind it (see `gui/auto_button.rs`). Borrowing the separator
        // stroke gives every widget at rest a quiet outline in whatever theme is in force.
        let mut visuals = Visuals::light();
        visuals.widgets.inactive.bg_stroke = visuals.widgets.noninteractive.bg_stroke;

        let style = Style {
            visuals,
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
                // Everything below only applies while the pointer is over the canvas: the wheel
                // and the middle button still mean what they usually do over the sidebar.
                let pointer_here = ui.rect_contains_pointer(ui.max_rect());

                // A plain wheel steps through the palette rather than scrolling — panning is the
                // middle button's job below — so the events have to be taken away from the
                // scroll area before it draws.
                if pointer_here {
                    self.wheel_remainder += take_wheel_notches(ctx);
                    let steps = self.wheel_remainder.trunc();
                    self.wheel_remainder -= steps;
                    if steps != 0.0 {
                        // A notch away from the user goes *up* the palette, the way it would move
                        // up any other list.
                        self.current_canvas().cycle_color(-steps as i32);
                    }
                }

                // Middle-drag pans, but only when there's somewhere to pan to; otherwise the
                // middle button keeps its painting job. A pan that has started holds on until
                // the button comes up, even if the pointer wanders off the canvas.
                let (middle_pressed, middle_down, pointer_delta) = ctx.input(|i| {
                    (
                        i.pointer.button_pressed(egui::PointerButton::Middle),
                        i.pointer.middle_down(),
                        i.pointer.delta(),
                    )
                });
                self.panning = middle_down && (self.panning || (middle_pressed && pointer_here));

                let (pannable, panning) = (self.pannable, self.panning);

                // Read out here, because the closure below borrows `self` mutably.
                let scale = self.render_scale();

                // A zoomed-in puzzle is routinely bigger than the window in both directions.
                // `animated(false)` because the only thing that scrolls this programmatically is
                // the pan below, which has to keep up with the pointer exactly.
                let scrolled = egui::ScrollArea::both().animated(false).show(ui, |ui| {
                    if panning {
                        ui.scroll_with_delta(pointer_delta);
                    }
                    if let Some(solve_gui) = &mut self.solve_gui {
                        solve_gui.canvas.middle_pans = pannable;
                        solve_gui.body(ui, scale);
                    } else {
                        self.editor_gui.middle_pans = pannable;
                        self.editor_gui.canvas(ui, scale, RenderStyle::Experimental);
                    }
                });

                // What the scroll area just measured decides whether the *next* frame's
                // middle-drag pans. A frame of lag is invisible beside how long a zoom lasts.
                self.pannable = scrolled.content_size.x > scrolled.inner_rect.width() + 0.5
                    || scrolled.content_size.y > scrolled.inner_rect.height() + 0.5;

                // After the canvas, which sets a cursor of its own for whichever tool is up.
                if panning {
                    ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
                }

                // egui routes ctrl-scroll (and trackpad pinch) into `zoom_delta` rather than
                // into the scroll offset, and `take_wheel_notches` leaves those events alone.
                if pointer_here {
                    let zoom = ui.input(|i| i.zoom_delta());
                    if zoom != 1.0 {
                        self.scale = (self.scale * zoom).clamp(MIN_SCALE, MAX_SCALE);
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

/// Drives `bt_solve::backtrack_solve` from a button: same shape as `Disambiguator` (a spawned
/// async task reporting back over channels, so the search can run in the background and be
/// stopped without freezing the GUI).
pub struct BacktrackSolver {
    /// The search's outcome, already formatted for display.
    report: Option<String>,
    pub terminate_s: mpsc::Sender<()>,
    progress_r: mpsc::Receiver<f32>,
    progress: f32,
    /// The formatted report, alongside the mask `canvas.rs` shades unsolved cells with — the same
    /// shape `editor_gui.solved_mask` already holds for the line-logic `Solve` button, so the two
    /// buttons can share it.
    report_r: mpsc::Receiver<(String, Option<Vec<bool>>)>,
}

impl Default for BacktrackSolver {
    fn default() -> Self {
        Self::new()
    }
}

impl BacktrackSolver {
    pub fn new() -> Self {
        BacktrackSolver {
            report: None,
            progress: 0.0,
            terminate_s: mpsc::channel().0,
            progress_r: mpsc::channel().1,
            report_r: mpsc::channel().1,
        }
    }

    pub fn widget(
        &mut self,
        picture: &DynSolution,
        version: Version,
        solved_mask: &mut Staleable<(String, Vec<bool>)>,
        progress: &SharedProgress,
        ui: &mut egui::Ui,
    ) {
        while let Ok(p) = self.progress_r.try_recv() {
            self.progress = p;
        }
        let running = self.progress > 0.0 && self.progress < 1.0;

        if let Ok((report, mask)) = self.report_r.try_recv() {
            // Shared with the line-logic `Solve` button: whichever one ran last is the one
            // shading the canvas, with no indication of which of the two it was.
            if let Some(mask) = mask {
                solved_mask.update((report.clone(), mask), version);
            }
            self.report = Some(report);
        }

        let (mut start, mut stop) = (false, false);
        ui.horizontal(|ui| {
            if !running {
                start = ui.button("Solve (backtracking)").clicked();
            } else {
                stop = ui.button("Stop").clicked();
            }
        });

        if start {
            let (p_s, p_r) = mpsc::channel();
            let (r_s, r_r) = mpsc::channel();
            let (t_s, t_r) = mpsc::channel();
            self.progress_r = p_r;
            self.terminate_s = t_s;
            self.report_r = r_r;
            self.report = None;

            let picture = picture.clone();
            spawn_async(async move {
                let puzzle = picture.to_puzzle();
                let outcome = crate::with_puzzle!(&puzzle, |p| {
                    bt_solve::backtrack_solve(p, &SolveOptions::default(), p_s, t_r).await
                });
                let (report, mask) = match outcome {
                    Ok(report) => (
                        format!(
                            "unsolved cells (upper bound): {}\n{}",
                            report.cells_left, report.solve_counts
                        ),
                        Some(report.solved_mask),
                    ),
                    Err(e) => (format!("{:?}", e), None),
                };
                let _ = r_s.send((report, mask));
            });
        }
        if stop {
            let _ = self.terminate_s.send(()); // Don't panic if it's already gone!
            self.progress = 0.0;
        }

        *progress.borrow_mut() = if running { Some(self.progress) } else { None };

        if let Some(report) = &self.report {
            ui.label(report);
        }
    }
}
