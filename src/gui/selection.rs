//! The lasso tool: drawing a loop, working out what it caught, and moving the catch around.
//!
//! Unlike the other tools (see `tools`), this one keeps tracking the pointer once it leaves the
//! grid — a loop drawn around the outside of a shape is perfectly ordinary — so it works in
//! abstract units rather than in cells, and `canvas_with_clues` hands it the pointer directly.

use super::*;

/// A lasso selection, plus whatever it has been lifted onto a floating layer.
///
/// The mask is stored at an *anchor* position and displaced by `offset`, rather than being
/// rewritten on every drag frame: that keeps a move reversible (drag back off the grid edge and
/// nothing is lost) and makes the whole thing one translation away from where it started.
pub struct Selection {
    /// Indexed by dense cell index, like `Solution::cells` — so it is only meaningful for a grid
    /// of the same size, which `canvas_with_clues` checks before using it.
    pub(super) mask: Vec<bool>,
    /// Lattice steps (see `Geometry::snap_translation`) currently applied to `mask` and
    /// `floating`. Zero until the selection is dragged.
    pub(super) offset: (i32, i32),
    /// Content lifted off the grid, as `(anchor cell, color)`. `None` until the first move drag.
    pub(super) floating: Option<Vec<(u32, Color)>>,
    /// The lasso path in abstract units, while one is being drawn.
    pub(super) drawing: Option<Vec<crate::layout::Point>>,
    /// Where the move drag was grabbed, and the offset at that moment.
    dragging: Option<(crate::layout::Point, (i32, i32))>,
    /// Time origin for the marching ants' dash phase.
    pub(super) since: Instant,
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
    pub(super) fn displayed_cells(&self, picture: &crate::puzzle::DynSolution) -> Vec<u32> {
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
pub(super) struct LassoPointer {
    pressed: bool,
    down: bool,
    released: bool,
}

impl LassoPointer {
    pub(super) fn from_egui(pointer: &egui::PointerState) -> LassoPointer {
        LassoPointer {
            pressed: pointer.button_pressed(egui::PointerButton::Primary),
            down: pointer.button_down(egui::PointerButton::Primary),
            released: pointer.any_released(),
        }
    }
}

impl CanvasGui {
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
    pub(super) fn lasso_input(&mut self, pointer: LassoPointer, p: Point) {
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
    pub(super) fn lasso_keys(&mut self, ui: &egui::Ui) {
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
    pub(super) fn lasso_cursor(&mut self, ui: &egui::Ui, hovered_cell: Option<u32>) {
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

/// The outline of a set of cells, as a list of abstract-unit segments.
///
/// Found by cancellation: push every selected cell's edges into a table, and an edge shared by
/// two selected cells lands there twice. What's left having landed once is exactly the boundary.
/// Works for squares and triangles.
pub(super) fn selection_outline(picture: &DynSolution, cells: &[u32]) -> Vec<(Point, Point)> {
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
pub(super) fn marching_ants(
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
