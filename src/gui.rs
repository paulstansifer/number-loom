mod annotate;
mod auto_button;
mod back_button;
mod canvas;
mod compact;
pub mod gallery;
mod input_mode;
mod outline_text;
mod palette;
mod resize;
mod selection;
mod session;
pub mod solver;
mod toolbar;
mod tools;
mod touch;
mod triano;
mod triddler;
mod view;

pub use crate::geometry::{CellIdx, FamilyIdx, LaneIdx, LanePos};
pub use annotate::{AnnotateDrag, Annotation};
pub use auto_button::{AutoButton, auto_button};
pub use canvas::{ClueId, ClueOverlay, HoverBlocks};
pub use compact::ScreenLayout;
pub use input_mode::InputMode;
pub use palette::default_color;
pub use selection::{Selection, cells_in_lasso};
pub use toolbar::LibraryStatus;
use toolbar::{NewPuzzleDialog, SaveShareDialog};
pub use tools::Tool;
pub use typed_index_collections::{TiSlice, TiVec};

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    sync::mpsc,
    time::Duration,
};

use serde::{Deserialize, Serialize};
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

/// The status bar's message, which whatever has something to say sets through a shared reference.
#[derive(Default)]
pub struct StatusCell {
    // The `Instant` is when the message was set: for the first `STATUS_GRACE_PERIOD` after that,
    // `maybe_clear_on_dirty` calls are ignored, so a message doesn't disappear before the user
    // has had a chance to read it.
    inner: RefCell<Option<(StatusMessage, Instant)>>,
}

impl StatusCell {
    pub fn set(&self, message: StatusMessage) {
        *self.inner.borrow_mut() = Some((message, Instant::now()));
    }

    pub fn get(&self) -> Option<StatusMessage> {
        self.inner
            .borrow()
            .as_ref()
            .map(|(message, _)| message.clone())
    }

    /// The message, and how long ago it was set.
    pub fn get_with_age(&self) -> Option<(StatusMessage, Duration)> {
        self.inner
            .borrow()
            .as_ref()
            .map(|(message, set_at)| (message.clone(), set_at.elapsed()))
    }

    /// Dismissed by the user.
    pub fn clear(&self) {
        *self.inner.borrow_mut() = None;
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

use solver::{RenderStyle, SolveGui};

use crate::{
    export::to_bytes,
    import,
    // The abstract-units point, distinct from egui's `Pos2`: everything the lasso does is in
    // grid space, and only the painter converts.
    layout::Point,
    puzzle::{
        BACKGROUND, ClueStyle, Color, ColorInfo, Document, DynSolution, Palette, PartialSolution,
        PuzzleDynOps, Solution, UNSOLVED,
    },
    solve::grid_solve::{self, DisambigResult, SolveOptions, disambig_candidates},
    solve::{conprop, line_solve::Cell},
    user_settings::{UserSettings, consts},
};
use egui::{Color32, Pos2, Rect, RichText, Shape, Style, TextStyle, Vec2, Visuals};

use egui_material_icons::icons;

/// The license a puzzle needs for it to be contributed to the library.
const CC_BY: &str = "CC BY 4.0";

/// Open the GUI on `document`. If its picture couldn't be worked out, it opens on a blank canvas,
/// and says why.
#[cfg(not(target_arch = "wasm32"))]
pub fn edit_image(document: anyhow::Result<Document>) {
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
            add_icon_font(&cc.egui_ctx);
            Ok(Box::new(NonogramGui::new_or_blank(document)))
        }),
    )
    .unwrap()
}

/// What `egui_material_icons::initialize` does, but against our own `egui`: that crate hasn't
/// caught up with it yet, so the only thing to take from it is the font (and the icon names).
pub fn add_icon_font(ctx: &egui::Context) {
    use egui::epaint::text::{FontInsert, FontPriority, InsertFontFamily};
    let mut data = egui::FontData::from_static(egui_material_icons::FONT_DATA);
    // That crate nudges its icons down by 0.05 em, which was right for egui 0.31; egui 0.37 sets
    // text a pixel lower by itself, so the nudge would leave the icons low.
    data.tweak.y_offset_factor = 0.0;
    ctx.add_font(FontInsert::new(
        "material-icons",
        data,
        vec![InsertFontFamily {
            family: egui::FontFamily::Proportional,
            priority: FontPriority::Lowest,
        }],
    ));
}

#[cfg(target_arch = "wasm32")]
pub fn edit_image(document: anyhow::Result<Document>) {
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

        let linked = woven_from_url_hash();
        if linked.is_some() {
            // Clear the puzzle link so it doesn't get stuck in the user's browser history!
            clear_url_hash();
        }
        let saved = session::load();

        let start_result = eframe::WebRunner::new()
            .start(
                canvas,
                web_options,
                Box::new(|cc| {
                    add_icon_font(&cc.egui_ctx);
                    let (link_document, link_error) = match linked {
                        Some(Ok(document)) => (Some(document), None),
                        Some(Err(e)) => (None, Some(e)),
                        None => (None, None),
                    };
                    let mut errors = vec![];
                    // A link opens on top of the saved session, which stays behind it in the
                    // editor's undo history (see `open_linked`).
                    let gui = match (link_document, saved) {
                        (Some(link_document), Some(Ok(mut gui))) => {
                            gui.open_linked(link_document);
                            gui.canvas
                                .status
                                .set(StatusMessage::info("Opened the puzzle at the link; 'undo' will restore your previous edits."));
                            gui
                        }
                        (Some(link_document), None) => NonogramGui::new_solving(link_document),
                        (None, Some(Ok(gui))) => gui,
                        (link_document, Some(Err(e))) => {
                            errors.push(format!("Unable to resume: {e}"));
                            match link_document {
                                Some(link_document) => NonogramGui::new_solving(link_document),
                                None => NonogramGui::new_or_blank(document),
                            }
                        }
                        (None, None) => NonogramGui::new_or_blank(document),
                    };
                    if let Some(e) = link_error {
                        errors.push(format!("Error loading the puzzle in the link: {e}"));
                    }
                    // The status bar holds one message, and both of these can happen at once.
                    if !errors.is_empty() {
                        gui.canvas
                            .status
                            .set(StatusMessage::error(errors.join("\n")));
                    }
                    Ok(Box::new(gui))
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

/// A link like `.../number-loom/#WOVEN-...-` carries a puzzle to open. `None` if the URL doesn't
/// have one.
#[cfg(target_arch = "wasm32")]
fn woven_from_url_hash() -> Option<anyhow::Result<Document>> {
    let hash = web_sys::window()?.location().hash().ok()?;
    // Some chat apps and mailers percent-encode the `+` and `/` of the base-64.
    let hash = js_sys::decode_uri_component(&hash)
        .ok()
        .and_then(|decoded| decoded.as_string())
        .unwrap_or(hash);
    let woven = hash.strip_prefix('#')?;
    if !woven.starts_with("WOVEN-") {
        return None;
    }
    Some(crate::formats::woven::from_woven(
        woven,
        "linked.woven".to_string(),
    ))
}

/// Take the `#...` off the address in the address bar, without reloading or adding to history.
#[cfg(target_arch = "wasm32")]
fn clear_url_hash() {
    let Some(window) = web_sys::window() else {
        return;
    };
    let location = window.location();
    let (Ok(path), Ok(search)) = (location.pathname(), location.search()) else {
        return;
    };
    if let Ok(history) = window.history() {
        let _ = history.replace_state_with_url(
            &wasm_bindgen::JsValue::NULL,
            "",
            Some(&format!("{path}{search}")),
        );
    }
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

/// Send `()` on `sender` once `delay` has passed, without waiting for it (e.g. to stop a
/// background search).
#[cfg(not(target_arch = "wasm32"))]
pub fn send_after(delay: web_time::Duration, sender: mpsc::Sender<()>) {
    std::thread::spawn(move || {
        std::thread::sleep(delay);
        let _ = sender.send(()); // Don't panic if it's already gone!
    });
}

#[cfg(target_arch = "wasm32")]
pub fn send_after(delay: web_time::Duration, sender: mpsc::Sender<()>) {
    use wasm_bindgen::JsCast as _;
    let callback = wasm_bindgen::closure::Closure::once_into_js(move || {
        let _ = sender.send(()); // Don't panic if it's already gone!
    });
    web_sys::window()
        .unwrap()
        .set_timeout_with_callback_and_timeout_and_arguments_0(
            callback.unchecked_ref(),
            delay.as_millis() as i32,
        )
        .expect("Failed to call set_timeout");
}

type Version = u32;

pub struct Staleable<T> {
    pub val: T,
    pub version: Version,
}

/// Never fresh, so the first `get_or_refresh` computes it. (What a skipped `Staleable` comes back
/// as when the session is restored.)
impl<T: Default> Default for Staleable<T> {
    fn default() -> Self {
        Staleable {
            val: T::default(),
            version: session::never(),
        }
    }
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

/// Saved with the session (see `session.rs`), except for the fields marked `#[serde(skip)]`, which
/// are either in the middle of something or worked out again from the rest.
#[derive(Serialize, Deserialize)]
pub struct CanvasGui {
    #[serde(with = "session::document")]
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
    #[serde(skip)]
    pub line_tool_state: Option<CellIdx>,
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
    #[serde(skip)]
    pub annotate_drag: Option<AnnotateDrag>,
    /// Whether this canvas is solving the puzzle rather than editing it. That decides what it
    /// shows and changes (the picture, or `in_progress`), which tools it offers (annotate is
    /// solver-only, the way flood fill and the lasso are editor-only), whether the palette can be
    /// edited, and whether a cell has an "unknown" state to go back to when a click undoes
    /// itself. See `set_solving`.
    pub solving: bool,
    /// The editor's own state while solving, or the solver's while editing.
    other_mode: ModeState,
    /// The palette editor's widget whose changes are going into the undo step on top of the
    /// stack (see `edit_color`).
    #[serde(skip)]
    color_edit: Option<egui::Id>,
    /// Whether the view has somewhere to pan to, in which case a middle-drag pans it and so must
    /// not also reach a tool. `main_ui` sets this from the scroll area's own measurements.
    #[serde(skip)]
    pub middle_pans: bool,
    /// In touch mode, where on screen the canvas takes touches from: the whole central panel,
    /// not just the picture, since the finger steers a cursor rather than pointing. `None` in
    /// mouse mode. `main_ui` sets this, as it does `middle_pans`.
    #[serde(skip)]
    pub touch_area: Option<Rect>,
    /// The touch cursor (see `touch.rs`).
    #[serde(skip)]
    pub(crate) touch: touch::TouchState,
    /// Indexed by dense cell index, like `Solution::cells`.
    #[serde(skip)]
    pub solved_mask: Staleable<(String, TiVec<CellIdx, bool>)>,
    #[serde(skip)]
    pub disambiguator: Staleable<Disambiguator>,
    #[serde(skip)]
    pub backtrack_solver: Staleable<BacktrackSolver>,
    #[serde(skip)]
    pub id: Staleable<String>,
    #[serde(skip)]
    pub status: StatusCell,
    /// For the progress bar in the status bar: `Some(fraction)` (0.0 to 1.0) while a
    /// long-running task is in progress, `None` otherwise.
    #[serde(skip)]
    pub progress: RefCell<Option<f32>>,
    /// Where the picture itself (not counting any clue gutters) was placed on screen, as of the
    /// last frame that drew it.
    #[serde(skip)]
    pub picture_rect: Option<Rect>,
    /// Whether to zoom to fit the picture (and any clues) on screen, because we're loading for
    /// the first time.
    #[serde(skip)]
    pub needs_fit: bool,
}

/// The whole app's state. Saved as the session (see `session.rs`), apart from what's marked
/// `#[serde(skip)]`: mostly dialogs, which a restored session starts with closed.
#[derive(Serialize, Deserialize)]
pub struct NonogramGui {
    // The `pub`s are solely for tests/gui.rs
    pub canvas: CanvasGui,
    /// Where the picture sits in the window.
    view: view::View,
    #[serde(skip, default = "session::dead_receiver")]
    opened_file_receiver: mpsc::Receiver<anyhow::Result<Document>>,
    #[serde(skip, default = "session::dead_receiver")]
    save_result_receiver: mpsc::Receiver<anyhow::Result<()>>,
    #[serde(skip, default = "session::dead_receiver")]
    library_receiver: mpsc::Receiver<anyhow::Result<Vec<Document>>>,
    #[serde(skip)]
    library_dialog: Option<LibraryStatus>,
    #[serde(skip)]
    new_dialog: Option<NewPuzzleDialog>,
    /// The editor's `Solve` button.
    line_solver: LineSolver,
    lines_to_affect_string: String,
    pub solve_gui: Option<SolveGui>,
    /// What the compact layout has open: the menu drawer, and its dialogs that don't belong to
    /// the desktop's toolbar.
    #[serde(skip)]
    compact: compact::CompactState,
    /// The browser's back button closes what's open, rather than leaving the app.
    #[serde(skip)]
    back_button: back_button::BackButton,
    /// The Save/share window, while it's open.
    #[serde(skip)]
    save_share: Option<SaveShareDialog>,
    /// Mouse or touch, by whichever was used last. Not saved: the first touch or hover after a
    /// restore says which.
    #[serde(skip)]
    input_mode: InputMode,
    /// How the last save of the session went. (Sessions are only saved on the web.)
    #[serde(skip)]
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    saving: session::SaveState,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Action {
    /// Keyed by dense cell index rather than by coordinate: every consumer wants a cell, and
    /// indices make undo, the tools, and merging work for any shape with no dispatch at all.
    /// Coordinates appear only at the hit-test boundary, as `DynCoord`.
    ChangeColor { changes: HashMap<CellIdx, Color> },
    ReplaceDocument {
        #[serde(with = "session::boxed_document")]
        document: Box<Document>,
    },
    /// Check a clue off in the solver's gutters, or un-check it. Its own inverse, so undo and
    /// redo are both just the same toggle again.
    ToggleClue { clue: ClueId },
    /// Set what a palette entry is called and how it looks, or with `None`, take it out of the
    /// palette altogether. (Only while no cell is that color: deleting a color that's in use
    /// repaints its cells too, which is a `ReplaceDocument`.)
    EditColor {
        color: Color,
        info: Option<ColorInfo>,
    },
}

/// What the editor and the solver each keep for themselves, though they share the canvas, and
/// with it the document. The one in use is in `CanvasGui`'s own fields, and the other is set aside
/// in `CanvasGui::other_mode` (see `CanvasGui::set_solving`).
#[derive(Serialize, Deserialize)]
struct ModeState {
    version: Version,
    current_color: Color,
    drag_start_color: Color,
    undo_stack: Vec<Action>,
    redo_stack: Vec<Action>,
    current_tool: Tool,
    previous_tool: Tool,
}

impl ModeState {
    fn new(palette: &Palette, tool: Tool) -> Self {
        let current_color = default_color(palette);
        ModeState {
            version: 0,
            current_color,
            drag_start_color: current_color,
            undo_stack: vec![],
            redo_stack: vec![],
            current_tool: tool,
            previous_tool: tool,
        }
    }
}

/// How an undecided cell is drawn. It's in no palette, since no picture has one (see `Document`).
static UNSOLVED_INFO: std::sync::LazyLock<ColorInfo> = std::sync::LazyLock::new(|| ColorInfo {
    ch: '?',
    name: "unknown".to_owned(),
    rgb: (128, 128, 128),
    color: UNSOLVED,
    corner: None,
});

/// How to draw `color`: as `palette` says, or for `UNSOLVED`, as `UNSOLVED_INFO` does.
pub(crate) fn color_info(palette: &Palette, color: Color) -> &ColorInfo {
    if color == UNSOLVED {
        &UNSOLVED_INFO
    } else {
        &palette[&color]
    }
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
    /// A canvas for editing `document` (see `set_solving` for solving it instead).
    fn new(document: Document) -> Self {
        let picture = document.solution();
        let solved_mask = vec![true; picture.cells().len()].into();
        let editor = ModeState::new(picture.palette(), Tool::Pencil);
        let solver = ModeState::new(picture.palette(), Tool::LineAlongLane);

        CanvasGui {
            document,
            version: editor.version,
            current_color: editor.current_color,
            drag_start_color: editor.drag_start_color,
            undo_stack: editor.undo_stack,
            redo_stack: editor.redo_stack,
            current_tool: editor.current_tool,
            previous_tool: editor.previous_tool,
            line_tool_state: None,
            selection: None,
            annotations: vec![],
            annotate_drag: None,
            checked_clues: HashSet::new(),
            solving: false,
            other_mode: solver,
            color_edit: None,
            middle_pans: false,
            touch_area: None,
            touch: Default::default(),
            picture_rect: None,
            needs_fit: true,
            solved_mask: Staleable {
                val: (String::new(), solved_mask),
                version: 0,
            },
            disambiguator: Staleable {
                val: Disambiguator::default(),
                version: 0,
            },
            backtrack_solver: Staleable {
                val: BacktrackSolver::default(),
                version: 0,
            },
            // Never fresh, so that the first frame names the document after its title, as every
            // change after that does.
            id: Staleable::default(),
            status: StatusCell::default(),
            progress: RefCell::new(None),
        }
    }

    /// Switch between editing the picture and solving it. Whatever progress there is towards
    /// solving it stays (in `Document::in_progress`) until the picture changes, along with the
    /// solver's undo history.
    pub(super) fn set_solving(&mut self, solving: bool) {
        if solving == self.solving {
            return;
        }
        if solving {
            // The lasso's floating layer belongs to the picture; put it down.
            self.clear_selection();
        }
        self.line_tool_state = None;
        self.annotate_drag = None;
        self.touch.gesture = Default::default();

        let mode = &mut self.other_mode;
        std::mem::swap(&mut self.version, &mut mode.version);
        std::mem::swap(&mut self.current_color, &mut mode.current_color);
        std::mem::swap(&mut self.drag_start_color, &mut mode.drag_start_color);
        std::mem::swap(&mut self.undo_stack, &mut mode.undo_stack);
        std::mem::swap(&mut self.redo_stack, &mut mode.redo_stack);
        std::mem::swap(&mut self.current_tool, &mut mode.current_tool);
        std::mem::swap(&mut self.previous_tool, &mut mode.previous_tool);
        self.solving = solving;

        if solving {
            self.document.progress_mut(); // Somewhere for the progress to go.
        }
        // The palette may have changed since this mode last had a look at it.
        self.clamp_colors_to_palette();
        // The solver's clue gutters take room that the editor has no use for.
        self.needs_fit = true;
    }

    /// One cell as the canvas shows it: the picture's color, or while solving, what's known of
    /// it so far (`UNSOLVED`, if it's not known for sure).
    pub fn cell(&self, cell: CellIdx) -> Color {
        if self.solving {
            self.document
                .in_progress()
                .and_then(|progress| progress[cell].known_or())
                .unwrap_or(UNSOLVED)
        } else {
            self.document.solution().cells()[cell]
        }
    }

    /// Every cell as the canvas shows it (see `cell`).
    pub fn cells(&self) -> std::borrow::Cow<'_, TiSlice<CellIdx, Color>> {
        if self.solving {
            let cells = self.document.solution().cells();
            std::borrow::Cow::Owned(cells.keys().map(|cell| self.cell(cell)).collect())
        } else {
            std::borrow::Cow::Borrowed(self.document.solution().cells())
        }
    }

    /// What's known while solving, as the solver's aids want it.
    pub fn progress(&self) -> PartialSolution {
        match self.document.in_progress() {
            Some(progress) if self.solving => progress.clone(),
            _ => self.document.solution().to_partial(),
        }
    }

    /// Change one cell as the canvas shows it (see `cell`), bumping `version` if that's a change.
    fn set_cell(&mut self, cell: CellIdx, color: Color) {
        if self.cell(cell) == color {
            return;
        }
        if self.solving {
            self.document.progress_mut()[cell] = if color == UNSOLVED {
                Cell::new_anything()
            } else {
                Cell::from_color(color)
            };
        } else {
            self.document.solution_mut().cells_mut()[cell] = color;
        }
        self.version += 1;
    }

    /// Sync `self.current_color` and `self.drag_start_color` to the document, for safety.
    fn clamp_colors_to_palette(&mut self) {
        let palette = self.document.solution().palette();
        let fallback = default_color(palette);
        let usable =
            |color: &Color| palette.contains_key(color) || (self.solving && *color == UNSOLVED);

        if !usable(&self.current_color) {
            self.current_color = fallback;
        }
        if !usable(&self.drag_start_color) {
            self.drag_start_color = fallback;
        }
    }

    fn reversed(&self, action: &Action) -> Action {
        match action {
            Action::ChangeColor { changes } => Action::ChangeColor {
                changes: changes
                    .keys()
                    .map(|cell| (*cell, self.cell(*cell)))
                    .collect(),
            },
            Action::ReplaceDocument { document: _ } => Action::ReplaceDocument {
                document: Box::new(self.document.clone()),
            },
            Action::ToggleClue { clue } => Action::ToggleClue { clue: *clue },
            Action::EditColor { color, info: _ } => Action::EditColor {
                color: *color,
                info: self.document.solution().palette().get(color).cloned(),
            },
        }
    }

    pub fn perform(&mut self, action: Action, mood: ActionMood) {
        use Action::*;
        use ActionMood::*;

        // A lifted selection's cells exist only in its floating layer, which replacing the
        // document drops (below). Stamp them back into this picture first, so that the copy of it
        // that undo keeps still has them.
        if mood == Normal && matches!(action, ReplaceDocument { .. }) {
            self.flatten_selection();
        }

        let version_before = self.version;
        let merging = matches!(mood, Merge | ReplaceAction)
            && matches!(
                (self.undo_stack.last(), &action),
                (Some(ChangeColor { .. }), ChangeColor { .. })
            );
        let editing_further = mood == Merge
            && matches!(
                (self.undo_stack.last(), &action),
                (Some(EditColor { color: was, .. }), EditColor { color, .. }) if was == color
            );
        if editing_further {
            // The undo entry already there puts back how the color was before any of this.
            self.apply(action);
        } else if merging {
            // Consecutive `ChangeColor`s can be merged with each other.
            let (
                Some(ChangeColor { changes }),
                ChangeColor {
                    changes: new_changes,
                },
            ) = (self.undo_stack.pop(), action)
            else {
                unreachable!("just checked");
            };
            let changes = self.merge(changes, new_changes, mood == ReplaceAction);
            self.undo_stack.push(ChangeColor { changes });
        } else {
            let reversed_action = self.reversed(&action);
            self.apply(action);
            match mood {
                // Unable to merge, so this is a new undo entry.
                Normal | Merge | ReplaceAction => {
                    self.undo_stack.push(reversed_action);
                    self.redo_stack.clear();
                }
                Undo => self.redo_stack.push(reversed_action),
                Redo => self.undo_stack.push(reversed_action),
            }
        }

        if self.version != version_before {
            self.status.maybe_clear_on_dirty();
            // A changed picture is a different puzzle. Its progress is already gone (see
            // `Document::solution_mut`), so the rest of the solve goes too.
            if !self.solving {
                self.other_mode.undo_stack.clear();
                self.other_mode.redo_stack.clear();
                self.annotations.clear();
                self.checked_clues.clear();
            }
        }
    }

    /// Fold `new_changes` into `changes`, the undo entry for the stroke in progress, making them
    /// on the way. If `replace`, the stroke now covers just `new_changes`, and whatever it
    /// covered before goes back to how it was.
    fn merge(
        &mut self,
        mut changes: HashMap<CellIdx, Color>,
        new_changes: HashMap<CellIdx, Color>,
        replace: bool,
    ) -> HashMap<CellIdx, Color> {
        if replace {
            for cell in new_changes.keys() {
                changes.entry(*cell).or_insert_with(|| self.cell(*cell));
            }
            changes.retain(|cell, old_color| {
                let kept = new_changes.contains_key(cell);
                if !kept {
                    self.set_cell(*cell, *old_color);
                }
                kept
            });
            for (cell, color) in new_changes {
                self.set_cell(cell, color);
            }
        } else {
            for (cell, color) in new_changes {
                if let std::collections::hash_map::Entry::Vacant(entry) = changes.entry(cell) {
                    entry.insert(self.cell(cell));
                    // Crucially, this only fires on a new cell!
                    // Otherwise, we'd be flipping cells back and forth as long as we
                    // were in them!
                    self.set_cell(cell, color);
                }
            }
        }
        changes
    }

    fn apply(&mut self, action: Action) {
        match action {
            Action::ChangeColor { changes } => {
                for (cell, color) in changes {
                    self.set_cell(cell, color);
                }
            }
            Action::ReplaceDocument { document } => {
                debug_assert!(!self.solving, "a new document goes to the editor");
                if document.solution().extent() != self.document.solution().extent() {
                    self.needs_fit = true;
                }
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
            }
            // No `version` bump: the picture is untouched, so nothing that watches it — the line
            // analysis, the solved mask, the replay — has any reason to recompute.
            Action::ToggleClue { clue } => {
                if !self.checked_clues.remove(&clue) {
                    self.checked_clues.insert(clue);
                }
            }
            // No `version` bump either: what a color is called or how it looks is no part of
            // the picture, so (among other things) it doesn't make solving start over.
            Action::EditColor { color, info } => match info {
                Some(info) => {
                    self.document.palette_mut().insert(color, info);
                }
                None => {
                    debug_assert!(!self.document.solution().cells().contains(&color));
                    self.document.palette_mut().remove(&color);
                    self.clamp_colors_to_palette();
                }
            },
        }
    }

    /// Change what `info.color` is called or how it looks, undoably, from the palette editor's
    /// `widget`. Typing a name or dragging around a color picker changes it on every frame, so
    /// all that one widget does goes into one undo step, until `start_color_edit` says a new
    /// one has begun.
    pub(super) fn edit_color(&mut self, info: ColorInfo, widget: egui::Id) {
        let mood = if self.color_edit == Some(widget) {
            ActionMood::Merge
        } else {
            ActionMood::Normal
        };
        self.color_edit = Some(widget);
        self.perform(
            Action::EditColor {
                color: info.color,
                info: Some(info),
            },
            mood,
        );
    }

    /// The next `edit_color` is a separate undo step, even from the same widget: the name field
    /// has just taken the keyboard, say, or the color picker has just opened.
    pub(super) fn start_color_edit(&mut self) {
        self.color_edit = None;
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
        self.undo_redo(ui, ScreenLayout::Desktop);

        ui.separator();

        self.tool_selector(ui, ScreenLayout::Desktop);
        self.clear_annotations_button(ui, ScreenLayout::Desktop);

        ui.separator();

        self.palette_editor(ui);
    }

    /// The undo and redo buttons, and their keys. On the desktop, each shows how many steps it
    /// has.
    pub(super) fn undo_redo(&mut self, ui: &mut egui::Ui, layout: ScreenLayout) {
        // A focused `TextEdit` reads its key events without consuming them, so a bare-key
        // shortcut still fires while the user is typing into one. Nothing here uses a modifier,
        // so every one of them has to be suppressed by hand.
        let typing = ui.ctx().egui_wants_keyboard_input();

        let (can_undo, can_redo) = (!self.undo_stack.is_empty(), !self.redo_stack.is_empty());
        let (undos, redos) = (self.undo_stack.len(), self.redo_stack.len());
        let desktop = layout == ScreenLayout::Desktop;

        let row = |ui: &mut egui::Ui| {
            if desktop {
                ui.label(format!("({undos})"));
            }
            let undo = ui
                .add_enabled(can_undo, compact::icon_button(icons::ICON_UNDO, layout))
                .clicked()
                || (can_undo && !typing && ui.input(|i| i.key_pressed(egui::Key::Z)));
            let redo = ui
                .add_enabled(can_redo, compact::icon_button(icons::ICON_REDO, layout))
                .clicked()
                || (can_redo && !typing && ui.input(|i| i.key_pressed(egui::Key::Y)));
            if desktop {
                ui.label(format!("({redos})"));
            }
            (undo, redo)
        };
        let (undo, redo) = match layout {
            ScreenLayout::Desktop => centered_row(ui, "undo_row", row),
            ScreenLayout::Compact => row(ui),
        };

        if undo {
            self.un_or_re_do(true);
        }
        if redo {
            self.un_or_re_do(false);
        }
    }

    /// Annotations aren't part of the picture, so undo can't take them back — this is the only
    /// way to be rid of them, and it only appears when there's something to clear.
    pub(super) fn clear_annotations_button(&mut self, ui: &mut egui::Ui, layout: ScreenLayout) {
        if !self.solving || self.annotations.is_empty() {
            return;
        }
        // See `undo_redo`.
        let typing = ui.ctx().egui_wants_keyboard_input();

        let button = |ui: &mut egui::Ui| {
            match layout {
                ScreenLayout::Desktop => ui.button("Clear annotations"),
                ScreenLayout::Compact => ui.add(compact::icon_button(icons::ICON_MOP, layout)),
            }
            .on_hover_text("Remove every annotation (press Escape)")
            .clicked()
        };
        let clicked = match layout {
            ScreenLayout::Desktop => centered_row(ui, "clear_annotations", button),
            ScreenLayout::Compact => button(ui),
        };

        if clicked || (!typing && ui.input(|i| i.key_pressed(egui::Key::Escape))) {
            self.annotations.clear();
            self.annotate_drag = None;
        }
    }
}

impl NonogramGui {
    /// `document`, or, if its picture couldn't be worked out, a blank canvas that says why.
    pub fn new_or_blank(document: anyhow::Result<Document>) -> Self {
        match document {
            Ok(document) => Self::new(document),
            Err(e) => {
                let gui = Self::new(Document::from_solution(
                    DynSolution::Square(Solution::blank_bw(20, 20)),
                    "blank.xml".to_string(),
                ));
                gui.canvas.status.set(StatusMessage::error(format!(
                    "could not solve this puzzle: {e:#}"
                )));
                gui
            }
        }
    }

    pub fn new(mut document: Document) -> Self {
        // (Public for testing)
        if document.author.is_empty()
            && let Some(author) = UserSettings::get(consts::EDITOR_AUTHOR_NAME)
        {
            document.author = author;
        }

        NonogramGui {
            canvas: CanvasGui::new(document),
            view: view::View::default(),
            opened_file_receiver: mpsc::channel().1,
            save_result_receiver: mpsc::channel().1,
            library_receiver: mpsc::channel().1,
            new_dialog: None,
            library_dialog: None,
            line_solver: LineSolver::new(),
            lines_to_affect_string: "5".to_string(),
            solve_gui: None,
            compact: Default::default(),
            back_button: Default::default(),
            save_share: None,
            input_mode: InputMode::default(),
            saving: session::SaveState::default(),
        }
    }

    fn edit_sidebar(&mut self, ui: &mut egui::Ui) {
        ui.vertical(|ui| {
            self.metadata_editor(ui);

            ui.separator();

            self.canvas.common_sidebar_items(ui);

            ui.separator();

            self.resize_controls(ui, ScreenLayout::Desktop);

            ui.separator();

            self.line_solver.widget(ui, &mut self.canvas);

            ui.separator();

            self.backtrack_controls(ui);

            ui.separator();

            self.disambig_controls(ui);
        });
    }

    /// What the editor does every frame, whichever layout is on screen and whatever it shows.
    fn editor_frame(&mut self) {
        // The id tracks the title, and `Save/share` needs it whether or not the title is on
        // screen (in a collapsed "Metadata" section, or a closed dialog).
        let backup_title = self.canvas.document.get_or_make_up_title();
        let id = self
            .canvas
            .id
            .get_or_refresh(self.canvas.version, || backup_title.clone());
        if self.canvas.document.id != *id {
            self.canvas.document.id = id.clone();
        }

        // The searches run in the background, and report back here. Each belongs to the picture
        // it was started on, so a change to the picture starts afresh with a new one.
        let canvas = &mut self.canvas;
        let version = canvas.version;
        let backtrack = canvas
            .backtrack_solver
            .get_or_refresh(version, BacktrackSolver::default);
        backtrack.poll(version, &mut canvas.solved_mask);
        let backtrack_progress = backtrack.running_progress();
        let disambiguator = canvas
            .disambiguator
            .get_or_refresh(version, Disambiguator::default);
        disambiguator.poll(&canvas.status);
        *canvas.progress.borrow_mut() = backtrack_progress.or(disambiguator.running_progress());
    }

    fn backtrack_controls(&mut self, ui: &mut egui::Ui) {
        let picture = self.canvas.document.solution();
        self.canvas.backtrack_solver.val.widget(picture, ui);
    }

    fn disambig_controls(&mut self, ui: &mut egui::Ui) {
        let picture = self.canvas.document.solution();
        self.canvas.disambiguator.val.widget(picture, ui);
    }

    /// The +/- buttons for each side of the picture.
    fn resize_controls(&mut self, ui: &mut egui::Ui, layout: ScreenLayout) {
        match self.canvas.document.solution().shape() {
            crate::geometry::Shape::Triangular(_) => self.tri_resizer(ui, layout),
            _ => self.resizer(ui, layout),
        }
    }

    /// Title, author, description and license; in a collapsable section.
    fn metadata_editor(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Metadata").show(ui, |ui| self.metadata_form(ui));
    }

    /// Title, author, description and license.
    fn metadata_form(&mut self, ui: &mut egui::Ui) {
        ui.add(egui::TextEdit::singleline(&mut self.canvas.document.title).hint_text("Title"));

        ui.horizontal(|ui| {
            ui.label("by ");
            if ui
                .add(
                    egui::TextEdit::singleline(&mut self.canvas.document.author)
                        .hint_text("Author"),
                )
                .changed()
            {
                let _ = UserSettings::set(consts::EDITOR_AUTHOR_NAME, &self.canvas.document.author);
            }
        });

        let description_label = ui.label("Description:");
        ui.text_edit_multiline(&mut self.canvas.document.description)
            .labelled_by(description_label.id);

        let mut is_cc_by = self.canvas.document.license == CC_BY;

        ui.label("License:");

        ui.horizontal(|ui| {
            let radio = ui.radio_value(&mut is_cc_by, true, "");
            if radio.changed() {
                self.canvas.document.license = CC_BY.to_string();
            };
            let link = ui.add(
                egui::Hyperlink::from_label_and_url(
                    CC_BY,
                    "https://creativecommons.org/licenses/by/4.0/",
                )
                .open_in_new_tab(true),
            );
            radio.labelled_by(link.id);
        });

        ui.horizontal(|ui| {
            let radio = ui.radio_value(&mut is_cc_by, false, "");
            if radio.changed() {
                self.canvas.document.license.clear();
            };
            let field = ui
                .add_enabled(
                    !is_cc_by,
                    egui::TextEdit::singleline(&mut self.canvas.document.license),
                )
                .accessible_name("Custom license");
            radio.labelled_by(field.id);
        });
    }

    /// Mouse or touch, by whichever was used last (see `InputMode::after`).
    pub fn input_mode(&self) -> InputMode {
        self.input_mode
    }

    /// Like `new`, but starting out in solve mode.
    pub fn new_solving(document: Document) -> Self {
        let mut gui = Self::new(document);
        gui.enter_solve_mode();
        gui
    }

    /// Open `document` (from a link) in solve mode, the way choosing it from the Library does:
    /// the document is replaced with an undoable `ReplaceDocument`, so undoing in the editor
    /// brings back whatever was there before, progress and all.
    pub fn open_linked(&mut self, document: Document) {
        self.replace_document(document);
        self.enter_solve_mode();
    }

    /// Replace the document, in the editor, undoably.
    fn replace_document(&mut self, document: Document) {
        self.exit_solve_mode();
        self.canvas.perform(
            Action::ReplaceDocument {
                document: Box::new(document),
            },
            ActionMood::Normal,
        );
    }

    /// Start solving, from whatever progress the document has (see `CanvasGui::set_solving`).
    fn enter_solve_mode(&mut self) {
        if self.solve_gui.is_none() {
            self.canvas.set_solving(true);
            self.solve_gui = Some(SolveGui::new(self.canvas.document.puzzle().clone()));
        }
    }

    /// Back to the editor. The progress stays with the document, to come back to, until the
    /// picture changes.
    fn exit_solve_mode(&mut self) {
        self.canvas.set_solving(false);
        self.solve_gui = None;
    }
}

/// Starting width of the tool sidebar; the user may adjust it.
const SIDEBAR_WIDTH: f32 = 150.0;

/// Lay out a row of widgets centerd within the available width.
///
/// egui won't do this on its own. `Layout::left_to_right` places items from the left edge
/// whatever its `main_align` says — `horizontal_placement` hardcodes `Align::LEFT` — and
/// `vertical_centered` only centers a child whose size it knows *before* laying it out, which a
/// `ui.horizontal` row is not. The obvious fix, a sizing pass followed by a real one, would mean
/// running `add_contents` twice per frame; these rows are made of buttons, so the measuring pass
/// would act on every click a second time.
///
/// So: pad by however wide the row turned out last frame, and record this frame's width for the
/// next one. Only the very first frame a row is shown is off-center.
fn centered_row<R>(
    ui: &mut egui::Ui,
    id_salt: &str,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let id = ui.scope_id().with(id_salt);
    let previous: Option<f32> = ui.data(|d| d.get_temp(id));
    let pad = previous.map_or(0.0, |w| ((ui.available_width() - w) / 2.0).max(0.0));

    let row = ui.horizontal(|ui| {
        ui.add_space(pad);
        add_contents(ui)
    });

    ui.data_mut(|d| d.insert_temp(id, row.response.rect.width() - pad));
    row.inner
}

/// egui's spacing, except where noted.
fn desktop_spacing() -> egui::Spacing {
    egui::Spacing {
        interact_size: Vec2::new(20.0, 20.0), // Used by the color-picker buttons
        // egui 0.37 doubled this, which makes the little "+"/"−" buttons wide, and the
        // auto-buttons' gear sliver with them. This is egui 0.31's default:
        button_padding: Vec2::new(4.0, 1.0),
        ..egui::Spacing::default()
    }
}

/// The whole app's style.
fn style_for(layout: ScreenLayout) -> Style {
    // egui leaves a button outline-less until it's hovered, which makes a button hard to tell
    // from a label until you go looking for it — and impossible to tell from a second button
    // parked right behind it (see `gui/auto_button.rs`). Borrowing the separator stroke gives
    // every widget at rest a quiet outline in whatever theme is in force.
    let mut visuals = Visuals::light();
    visuals.widgets.inactive.bg_stroke = visuals.widgets.noninteractive.bg_stroke;

    // egui 0.37 rounded its widgets' corners off to 4–6. These are egui 0.31's defaults;
    // I think they look less busy in the tightly-packed Number Loom UI.
    let widgets = &mut visuals.widgets;
    for (state, radius) in [
        (&mut widgets.noninteractive, 2),
        (&mut widgets.inactive, 2),
        (&mut widgets.hovered, 3),
        (&mut widgets.active, 2),
        (&mut widgets.open, 2),
    ] {
        state.corner_radius = egui::CornerRadius::same(radius);
    }

    let (visuals, spacing, text_styles) = match layout {
        ScreenLayout::Desktop => (visuals, desktop_spacing(), Style::default().text_styles),
        ScreenLayout::Compact => (
            compact::visuals(visuals),
            compact::spacing(),
            compact::text_styles(),
        ),
    };

    Style {
        visuals,
        spacing,
        text_styles,

        ..Style::default()
    }
}

impl eframe::App for NonogramGui {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.main_ui(ui);

        self.saving.dirty = true;
        // The restored session came out of storage as it was loaded (see `session::load`). Now
        // that a frame has been drawn from it without incident, put it back.
        #[cfg(target_arch = "wasm32")]
        if !self.saving.attempted {
            self.save_session();
        }
    }

    /// On the web, eframe calls this every `auto_save_interval` and whenever the page loses
    /// focus. (It means to call it before the page unloads, too, but listens for an event called
    /// `onbeforeunload`, which never fires: the event is `beforeunload`.) eframe's own `storage`
    /// is no use here: it throws away the error when a write fails, and running out of room is
    /// exactly the failure that needs handling.
    fn save(&mut self, _storage: &mut dyn eframe::Storage) {
        #[cfg(target_arch = "wasm32")]
        self.save_session();
    }
}

impl NonogramGui {
    /// The whole window, panel by panel, in the root `ui`. Separate from `eframe::App::ui` for
    /// testing.
    pub fn main_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = &ui.ctx().clone();
        self.input_mode = ctx.input(|i| self.input_mode.after(i));
        let layout = ScreenLayout::of(ctx);

        // Styling. Has to be here instead of `edit_image` to take effect on the Web. The root
        // `ui` took its style from the context before this ran, so it needs telling too.
        let style = std::sync::Arc::new(style_for(layout));
        ctx.set_global_style(style.clone());
        ui.set_style(style);

        // Whatever is on screen, the mode's background work goes on.
        match &mut self.solve_gui {
            Some(solve_gui) => solve_gui.solver_frame(&self.canvas),
            None => self.editor_frame(),
        }

        for _ in 0..self.back_button.take_presses(ctx) {
            if let Some(&topmost) = self.overlays(layout).first() {
                self.close_overlay(topmost);
            }
        }

        match layout {
            ScreenLayout::Desktop => self.desktop_ui(ui),
            ScreenLayout::Compact => self.compact_ui(ui),
        }

        let open = self.overlays(layout).len();
        self.back_button.sync(open);
    }

    /// The desktop layout: a toolbar, a status bar, a sidebar, and the canvas.
    fn desktop_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = &ui.ctx().clone();
        // Panel order matters: egui hands each panel the space its predecessors didn't claim, so
        // the top and bottom bars span the full width, and the sidebar then splits what's left
        // with the canvas.
        egui::Panel::top("toolbar").show(ui, |ui| {
            self.toolbar(ctx, ui);
        });
        self.dialogs(ctx, ScreenLayout::Desktop);

        egui::Panel::bottom("status_bar").show(ui, |ui| {
            ui.horizontal(|ui| {
                // Reserves a consistent height for the bar even when there's nothing to show,
                // so the rest of the UI doesn't jump around as messages come and go.
                ui.label("");

                if let Some(progress) = *self.canvas.progress.borrow() {
                    // ~50% wider than the sidebar (150.0) is by default.
                    ui.add(
                        egui::ProgressBar::new(progress)
                            .animate(true)
                            .desired_width(225.0),
                    );
                }

                if let Some(status) = self.canvas.status.get() {
                    let color = if status.is_error {
                        Color32::DARK_RED
                    } else {
                        ui.visuals().text_color()
                    };
                    ui.colored_label(color, &status.text);
                }
            });
        });

        egui::Panel::left("sidebar")
            .resizable(true)
            .default_size(SIDEBAR_WIDTH)
            .size_range(SIDEBAR_WIDTH..=400.0)
            .show(ui, |ui| {
                // Both sidebars can outgrow a short window — the editor's once "Metadata" is
                // expanded, the solver's once a puzzle has a long palette.
                egui::ScrollArea::vertical().show(ui, |ui| {
                    if let Some(solve_gui) = &mut self.solve_gui {
                        solve_gui.sidebar(ui, &mut self.canvas);
                    } else {
                        self.edit_sidebar(ui);
                    }
                });
            });

        self.canvas_panel(ui, ScreenLayout::Desktop);
    }
}

/// A search running in the background (see `spawn_async`), so that the GUI neither freezes nor
/// waits for it: it reports how far it's got as it goes and then its result, and it can be told to
/// stop early. `Disambiguator` and `BacktrackSolver` each drive one.
pub struct BackgroundSearch<R> {
    running: bool,
    progress: f32,
    progress_r: mpsc::Receiver<f32>,
    result_r: mpsc::Receiver<R>,
    terminate_s: mpsc::Sender<()>,
}

/// The search's own ends of a `BackgroundSearch`'s channels.
pub struct SearchEnds<R> {
    pub progress: mpsc::Sender<f32>,
    pub terminate: mpsc::Receiver<()>,
    pub result: mpsc::Sender<R>,
}

impl<R> Default for BackgroundSearch<R> {
    fn default() -> Self {
        BackgroundSearch {
            running: false,
            progress: 0.0,
            progress_r: session::dead_receiver(),
            result_r: session::dead_receiver(),
            terminate_s: mpsc::channel().0,
        }
    }
}

/// A change to the picture replaces whatever owns the search (see `NonogramGui::editor_frame`),
/// and then nobody is left to hear what it would have found.
impl<R> Drop for BackgroundSearch<R> {
    fn drop(&mut self) {
        self.stop();
    }
}

impl<R> BackgroundSearch<R> {
    /// Get ready for a new search, stopping any that's under way. The search itself is then
    /// spawned with the ends this returns.
    pub fn start(&mut self) -> SearchEnds<R> {
        self.stop();
        let (progress, progress_r) = mpsc::channel();
        let (result, result_r) = mpsc::channel();
        let (terminate_s, terminate) = mpsc::channel();
        self.progress_r = progress_r;
        self.result_r = result_r;
        self.terminate_s = terminate_s;
        self.running = true;
        self.progress = 0.0;
        SearchEnds {
            progress,
            terminate,
            result,
        }
    }

    pub fn running(&self) -> bool {
        self.running
    }

    /// How far along it is, while it's running.
    pub fn running_progress(&self) -> Option<f32> {
        self.running.then_some(self.progress)
    }

    /// Take in whatever the search has reported since last time, and its result, if that's
    /// arrived. A search that has been stopped still reports what it found before it stopped.
    pub fn poll(&mut self) -> Option<R> {
        while let Ok(p) = self.progress_r.try_recv() {
            self.progress = p;
        }
        let result = self.result_r.try_recv().ok()?;
        self.running = false;
        Some(result)
    }

    pub fn stop(&mut self) {
        let _ = self.terminate_s.send(()); // Don't panic if it's already gone!
        self.running = false;
    }
}

/// Drives `grid_solve::disambig_candidates` from a button.
#[derive(Default)]
pub struct Disambiguator {
    /// Indexed by dense cell index, like `Solution::cells`.
    report: Option<TiVec<CellIdx, (Color, f32)>>,
    search: BackgroundSearch<DisambigResult>,
}

impl Disambiguator {
    fn running(&self) -> bool {
        self.search.running()
    }

    /// How far along it is, while it's running.
    pub fn running_progress(&self) -> Option<f32> {
        self.search.running_progress()
    }

    /// Collect whatever the search has sent back since last time.
    pub fn poll(&mut self, status: &StatusCell) {
        if let Some(result) = self.search.poll() {
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
    }

    pub fn start(&mut self, picture: &DynSolution) {
        let SearchEnds {
            progress,
            terminate,
            result,
        } = self.search.start();
        let solution = picture.clone();
        spawn_async(async move {
            let _ = result.send(disambig_candidates(&solution, progress, terminate).await);
        });
    }

    pub fn stop(&mut self) {
        self.search.stop();
    }

    /// The desktop's buttons. (`poll` has already taken in whatever there was, so "Clear"
    /// becomes available on the same frame the report lands, rather than the one after.)
    pub fn widget(&mut self, picture: &DynSolution, ui: &mut egui::Ui) {
        let running = self.running();

        // Both buttons share a row: "Clear" discards what the button beside it produced. They
        // only record what was clicked, since acting on it needs `self` mutably.
        let (mut start, mut stop, mut clear) = (false, false, false);
        ui.horizontal(|ui| {
            if !running {
                start = ui.button("Disambiguate!").clicked();
            } else {
                stop = ui.button("Stop").clicked();
            }
            clear = ui
                .add_enabled(self.report.is_some(), egui::Button::new("Clear"))
                .clicked();
        });

        if start {
            self.start(picture);
        }
        if stop {
            self.stop();
        }
        if clear {
            self.report = None;
        }
    }
}

/// Drives line logic (`DynPuzzle::line_solve`) from the editor's `Solve` button, or after every
/// change: whether it gets all the way says whether the puzzle can be solved without guessing.
/// It shades the cells it can't reach by way of `CanvasGui::solved_mask`.
#[derive(Serialize, Deserialize)]
pub struct LineSolver {
    /// Whether it runs after every change. A user setting, too (see `set_auto`).
    auto: bool,
    /// What it found, for showing under the button.
    #[serde(skip)]
    report: String,
    /// The `canvas.version` `report` was computed for, if any — independent of
    /// `canvas.solved_mask`'s own freshness, which the backtracking solve also writes to
    /// (see `BacktrackSolver::poll`). Without this, `solved_mask.get_or_refresh` would see
    /// backtracking's fresher write and skip re-running line logic, leaving `report` showing
    /// backtracking's text under the `Solve` button.
    #[serde(skip)]
    report_version: Option<Version>,
    /// How many cells the line solve behind `report` left unsolved.
    #[serde(skip)]
    cells_left: Option<usize>,
}

impl LineSolver {
    fn new() -> Self {
        LineSolver {
            auto: UserSettings::get_bool(consts::EDITOR_AUTO_SOLVE),
            report: String::new(),
            report_version: None,
            cells_left: None,
        }
    }

    /// Whether it runs after every change.
    pub(super) fn auto(&self) -> bool {
        self.auto
    }

    /// Switch running after every change on or off, and remember it.
    pub(super) fn set_auto(&mut self, auto: bool) {
        self.auto = auto;
        let _ = UserSettings::set(consts::EDITOR_AUTO_SOLVE, &auto.to_string());
        if !auto {
            // The shading clears itself (it's only drawn while fresh), but the report is plain
            // text that would otherwise linger after the aid is switched off.
            self.report.clear();
            self.report_version = None;
            self.cells_left = None;
        }
    }

    /// Whether what it last found is about the picture as it now stands.
    fn fresh(&self, canvas: &CanvasGui) -> bool {
        self.report_version == Some(canvas.version)
    }

    /// How many cells line logic leaves unsolved in the picture as it now stands, if that's
    /// known.
    pub(super) fn cells_left(&self, canvas: &CanvasGui) -> Option<usize> {
        self.cells_left.filter(|_| self.fresh(canvas))
    }

    /// Run if `requested`, or if it runs after every change, unless it's already up to date.
    pub(super) fn update(&mut self, canvas: &mut CanvasGui, requested: bool) {
        if !(requested || self.auto) || self.fresh(canvas) {
            return;
        }
        let puzzle = canvas.document.solution().to_puzzle();

        let mask = match puzzle.line_solve() {
            Ok(grid_solve::Report {
                solve_counts,
                cells_left,
                solved_mask,
                ..
            }) => {
                self.cells_left = Some(cells_left);
                // Unsolved cells first: that's the number that says whether the puzzle works.
                // The skim/scrub counts are solver diagnostics.
                self.report = format!("unsolved cells: {cells_left}\n{solve_counts}");
                Some(solved_mask)
            }
            // Can't happen, because the puzzle has a solution!
            Err(e) => {
                self.cells_left = None;
                self.report = format!("Error: {e:#}");
                None
            }
        };
        self.report_version = Some(canvas.version);
        if let Some(mask) = mask {
            canvas
                .solved_mask
                .update((self.report.clone(), mask), canvas.version);
        }
    }

    /// The desktop's button, its "after every change" toggle, and the report.
    fn widget(&mut self, ui: &mut egui::Ui, canvas: &mut CanvasGui) {
        let solve = auto_button(ui, "Solve", &mut self.auto);
        if solve.auto.changed() {
            self.set_auto(self.auto);
        }
        self.update(canvas, solve.button.clicked());

        ui.colored_label(
            if self.fresh(canvas) {
                Color32::BLACK
            } else {
                Color32::GRAY
            },
            &self.report,
        );
    }
}

/// Drives `conprop::conprop_solve_in_background` from a button.
#[derive(Default)]
pub struct BacktrackSolver {
    /// The search's outcome, already formatted for display.
    report: Option<String>,
    /// The formatted report, alongside the mask `canvas.rs` shades unsolved cells with — the same
    /// shape `canvas.solved_mask` already holds for the line-logic `Solve` button, so the two
    /// buttons can share it.
    search: BackgroundSearch<(String, Option<TiVec<CellIdx, bool>>)>,
}

impl BacktrackSolver {
    /// How far along it is, while it's running.
    pub fn running_progress(&self) -> Option<f32> {
        self.search.running_progress()
    }

    /// Collect whatever the search has sent back since last time.
    pub fn poll(
        &mut self,
        version: Version,
        solved_mask: &mut Staleable<(String, TiVec<CellIdx, bool>)>,
    ) {
        if let Some((report, mask)) = self.search.poll() {
            // Shared with the line-logic `Solve` button: whichever one ran last is the one
            // shading the canvas, with no indication of which of the two it was.
            if let Some(mask) = mask {
                solved_mask.update((report.clone(), mask), version);
            }
            self.report = Some(report);
        }
    }

    pub fn start(&mut self, picture: &DynSolution) {
        let SearchEnds {
            progress,
            terminate,
            result,
        } = self.search.start();
        self.report = None;

        let picture = picture.clone();
        spawn_async(async move {
            let puzzle = picture.to_puzzle();
            let outcome = crate::with_puzzle!(&puzzle, |p| {
                conprop::conprop_solve_in_background(
                    p,
                    &SolveOptions::default(),
                    progress,
                    terminate,
                )
                .await
            });
            let (report, mask) = match outcome {
                Ok(report) => (
                    format!(
                        "unsolved cells (upper bound): {}\n{}",
                        report.cells_left, report.solve_counts
                    ),
                    Some(report.solved_mask),
                ),
                Err(e) => (format!("{e:#}"), None),
            };
            let _ = result.send((report, mask));
        });
    }

    pub fn stop(&mut self) {
        self.search.stop();
    }

    /// The button, and the report once there is one.
    pub fn widget(&mut self, picture: &DynSolution, ui: &mut egui::Ui) {
        let running = self.search.running();

        let (mut start, mut stop) = (false, false);
        ui.horizontal(|ui| {
            if !running {
                start = ui.button("Solve (backtracking)").clicked();
            } else {
                stop = ui.button("Stop").clicked();
            }
        });

        if start {
            self.start(picture);
        }
        if stop {
            self.stop();
        }

        if let Some(report) = &self.report {
            ui.label(report);
        }
    }
}

#[cfg(test)]
mod background_search_tests {
    use super::*;

    /// Running from the moment it starts, rather than from its first progress report, and until
    /// its result is in.
    #[test]
    fn a_search_runs_from_its_start_to_its_result() {
        let mut search = BackgroundSearch::<u32>::default();
        assert_eq!(search.running_progress(), None);

        let ends = search.start();
        assert_eq!(search.running_progress(), Some(0.0));
        ends.progress.send(0.5).unwrap();
        assert_eq!(search.poll(), None);
        assert_eq!(search.running_progress(), Some(0.5));

        ends.result.send(7).unwrap();
        assert_eq!(search.poll(), Some(7));
        assert!(!search.running());
    }

    /// What replacing a stale `Disambiguator` or `BacktrackSolver` does to its search.
    #[test]
    fn dropping_a_search_stops_it() {
        let mut search = BackgroundSearch::<u32>::default();
        let ends = search.start();
        drop(search);
        assert!(ends.terminate.try_recv().is_ok());
    }

    /// Starting over stops the search already under way, and its result goes nowhere.
    #[test]
    fn starting_again_stops_the_first_search() {
        let mut search = BackgroundSearch::<u32>::default();
        let first = search.start();
        let second = search.start();
        assert!(first.terminate.try_recv().is_ok());
        assert!(first.result.send(1).is_err());

        second.result.send(2).unwrap();
        assert_eq!(search.poll(), Some(2));
    }
}

#[cfg(test)]
mod editor_tests {
    use super::*;

    /// The line solve runs when it's asked to, or after every change if it's set to, and only
    /// says how many cells it left while that's still about the picture as it stands.
    #[test]
    fn the_line_solve_only_reports_on_the_current_picture() {
        let mut canvas = NonogramGui::new(Document::from_solution(
            DynSolution::Square(Solution::blank_bw(3, 3)),
            "test".to_string(),
        ))
        .canvas;
        // Not `LineSolver::new`, or `set_auto`: those touch the user's settings.
        let mut solver = LineSolver {
            auto: false,
            report: String::new(),
            report_version: None,
            cells_left: None,
        };

        solver.update(&mut canvas, false);
        assert_eq!(solver.cells_left(&canvas), None, "not asked to");
        solver.update(&mut canvas, true);
        assert_eq!(solver.cells_left(&canvas), Some(0));

        canvas.perform(
            Action::ChangeColor {
                changes: [(CellIdx(4), Color(1))].into(),
            },
            ActionMood::Normal,
        );
        assert_eq!(solver.cells_left(&canvas), None, "out of date");
        solver.auto = true;
        solver.update(&mut canvas, false);
        assert_eq!(solver.cells_left(&canvas), Some(0));
    }

    /// As it is after every change: the id is the title.
    #[test]
    fn the_document_is_named_after_its_title_from_the_start() {
        let mut doc = Document::from_solution(
            DynSolution::Square(Solution::blank_bw(3, 3)),
            "test".to_string(),
        );
        doc.title = "Teapot".to_string();
        let mut gui = NonogramGui::new(doc);
        gui.editor_frame();
        assert_eq!(gui.canvas.document.id, "Teapot");
    }
}

/// The editor and the solver share the one canvas, and switch between them with `set_solving`.
#[cfg(test)]
mod mode_tests {
    use super::*;

    /// A 3x3 with its top row filled.
    fn gui() -> NonogramGui {
        let mut picture = Solution::blank_bw(3, 3);
        for x in 0..3 {
            picture.cells[CellIdx(x)] = Color(1);
        }
        NonogramGui::new(Document::from_solution(
            DynSolution::Square(picture),
            "test".to_string(),
        ))
    }

    fn paint(gui: &mut NonogramGui, cell: u32, color: Color) {
        gui.canvas.perform(
            Action::ChangeColor {
                changes: [(CellIdx(cell), color)].into(),
            },
            ActionMood::Normal,
        );
    }

    /// The editor shows the picture and the solver the progress, which is never in the picture.
    #[test]
    fn solving_paints_the_progress_not_the_picture() {
        let mut gui = gui();
        gui.enter_solve_mode();
        assert_eq!(gui.canvas.cell(CellIdx(0)), UNSOLVED);
        paint(&mut gui, 4, BACKGROUND);
        assert_eq!(gui.canvas.cell(CellIdx(4)), BACKGROUND);

        gui.exit_solve_mode();
        assert_eq!(gui.canvas.cell(CellIdx(0)), Color(1));
        assert!(!gui.canvas.document.solution().cells().contains(&UNSOLVED));
    }

    /// Progress made while solving is still there after a look at the editor, undo and all.
    #[test]
    fn progress_survives_a_trip_to_the_editor() {
        let mut gui = gui();
        gui.enter_solve_mode();
        paint(&mut gui, 0, Color(1));
        gui.exit_solve_mode();
        gui.enter_solve_mode();
        assert_eq!(gui.canvas.cell(CellIdx(0)), Color(1));

        gui.canvas.un_or_re_do(true);
        assert_eq!(gui.canvas.cell(CellIdx(0)), UNSOLVED);
    }

    /// Changing the picture changes the puzzle, so the solve starts over: no progress, and no
    /// history of it to undo.
    #[test]
    fn editing_the_picture_starts_the_solve_over() {
        let mut gui = gui();
        gui.enter_solve_mode();
        paint(&mut gui, 0, Color(1));
        gui.exit_solve_mode();
        paint(&mut gui, 8, Color(1));
        gui.enter_solve_mode();
        assert_eq!(gui.canvas.cell(CellIdx(0)), UNSOLVED);
        assert!(gui.canvas.undo_stack.is_empty());

        // The editor's own history is untouched.
        gui.exit_solve_mode();
        assert_eq!(gui.canvas.undo_stack.len(), 1);
    }
}

#[cfg(test)]
mod new_solving_tests {
    use super::*;

    #[test]
    fn a_linked_puzzle_opens_in_solve_mode() {
        let woven = include_str!("../examples/woven/square_bw.1.woven");
        let doc =
            crate::formats::woven::from_woven(woven.trim(), "linked.woven".to_string()).unwrap();
        assert!(NonogramGui::new_solving(doc).solve_gui.is_some());
    }

    #[test]
    fn a_linked_puzzle_opens_with_its_progress() {
        use crate::solve::line_solve::Cell;

        let woven = include_str!("../examples/woven/square_bw.1.woven");
        let mut doc =
            crate::formats::woven::from_woven(woven.trim(), "linked.woven".to_string()).unwrap();
        let answer = doc.solution().cells().raw.to_vec();
        // The first cell, as it is in the answer, and nothing else.
        let mut progress: PartialSolution = vec![Cell::new_anything(); answer.len()].into();
        progress[CellIdx(0)] = Cell::from_color(answer[0]);
        doc.set_in_progress(Some(progress));

        let woven = crate::formats::woven::to_woven(&doc).unwrap();
        let doc = crate::formats::woven::from_woven(&woven, "linked.woven".to_string()).unwrap();
        let gui = NonogramGui::new_solving(doc);
        let grid = gui.canvas.cells().raw.to_vec();
        assert_eq!(grid[0], answer[0]);
        assert!(grid[1..].iter().all(|&c| c == UNSOLVED));
    }
}
