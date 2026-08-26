//! The annotate tool, to help counting-out lines during a solve.
//!
//! A drag runs cell to cell along a lane: it ticks the border past each end and writes how many
//! cells it covered. It's anchored on the *cell* the drag started from rather than on a border,
//! so it can swing to any lane through that cell — four directions on a square grid, six on a
//! triddler — and reversing the drag flips the origin's tick to that cell's other side. A click
//! clears every mark covering the cell clicked on, or marks that one cell if there was nothing
//! there to clear.
//!
//! This doesn't affect the puzzle, and is invisible to the undo system.
//!
//! Everything is stated in terms of lanes rather than coordinates, the way `grid_solve` is, so a
//! triddler needs no special handling: the cell count of a mark is just how far apart its two
//! ends are.

use super::*;

/// A boundary within a lane: the gap just before `lane.cells[index]`, with `index == len` meaning
/// the far end. A lane of n cells therefore has n + 1 borders.
///
/// Only ever reached through an `Annotation`'s two ends; the tool itself snaps to cells.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Border {
    lane: usize,
    index: usize,
}

/// One scratch mark, running cell to cell along a lane.
///
/// Stored as the pair of *borders* enclosing the run — positions in `0..=lane.cells.len()`, never
/// equal, so a mark always covers at least one cell. Borders rather than the cells themselves,
/// because the pair also carries the direction the drag ran, which a one-cell mark would
/// otherwise lose: `from` is the border past the origin cell on the far side from where the drag
/// was heading, and the wave goes on the right-hand side of `from -> to`. So the two ends are
/// deliberately not sorted.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Annotation {
    lane: usize,
    from: usize,
    to: usize,
}

impl Annotation {
    /// The two borders the mark is drawn between, in drag order.
    fn ends(&self) -> (Border, Border) {
        let border = |index| Border {
            lane: self.lane,
            index,
        };
        (border(self.from), border(self.to))
    }

    /// A mark on a single cell, as a click leaves.
    ///
    /// One cell has no direction, so the lane this ends up filed under is only a way of naming
    /// the cell — any lane through it would do, and the drawing ignores the choice entirely.
    fn lone(picture: &DynSolution, cell: u32) -> Option<Annotation> {
        let membership = picture.lane_map().memberships(cell).first()?;
        let position = membership.position as usize;
        Some(Annotation {
            lane: membership.lane as usize,
            from: position,
            to: position + 1,
        })
    }

    /// The one cell this mark covers, if it covers only the one.
    ///
    /// Such a mark is drawn as a box around that cell rather than as a run between two ticks:
    /// with nothing to count along, the direction it was made in isn't worth showing, and a
    /// click — which is how most of them are made — has no direction to show in the first place.
    fn lone_cell(&self, picture: &DynSolution) -> Option<u32> {
        if self.cells_covered() != 1 {
            return None;
        }
        let lane = picture.lane_map().lanes().get(self.lane)?;
        lane.cells.get(self.from.min(self.to)).copied()
    }

    /// How many cells this mark covers; always at least one.
    pub fn cells_covered(&self) -> usize {
        self.from.abs_diff(self.to)
    }

    /// Whether `cell` is one of the cells this mark covers — what a click tests against.
    ///
    /// Cell `i` of a lane sits between borders `i` and `i + 1`, so the covered cells are the
    /// half-open range between the two ends.
    fn covers(&self, picture: &DynSolution, cell: u32) -> bool {
        picture
            .lane_map()
            .memberships(cell)
            .iter()
            .find(|m| m.lane as usize == self.lane)
            .is_some_and(|m| {
                let position = m.position as usize;
                self.from.min(self.to) <= position && position < self.from.max(self.to)
            })
    }
}

/// An annotation being dragged out right now.
pub struct AnnotateDrag {
    /// The cell the drag started on. The mark swings around this one.
    origin: u32,
    /// Where the press landed, in abstract units. The drag is measured from here rather than
    /// from the origin cell's centre, so the mark answers to the pointer directly.
    press: Point,
    /// What would be committed if the pointer were released right now — `None` until the pointer
    /// has moved far enough for this to be a drag rather than a click.
    live: Option<Annotation>,
}

/// What the annotate tool needs to know about the pointer in a frame. It doesn't care *which*
/// button: annotating never paints, so every button can drive it.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct AnnotatePointer {
    pressed: bool,
    down: bool,
    released: bool,
    /// egui's own verdict on whether this has become a drag rather than a click.
    dragging: bool,
}

impl AnnotatePointer {
    pub(super) fn from_egui(pointer: &egui::PointerState) -> AnnotatePointer {
        AnnotatePointer {
            pressed: pointer.any_pressed(),
            down: pointer.any_down(),
            released: pointer.any_released(),
            dragging: pointer.is_decidedly_dragging(),
        }
    }
}

/// The cell edge across which position along a `family` lane changes: the one leading to the
/// previous cell in that lane when `near`, and to the next when not.
///
/// A square cell has one of each, and the family that *changes* across them is the other of the
/// two — a row's cells are divided by the edges that separate columns. A triangular cell's three
/// edges cover the three families one each, so the edge wanted here belongs to one of the two
/// families that aren't the lane's own, and [`CellShape::triangle_edge_is_near`] says which of
/// those two is on the near side.
pub(super) fn lane_step_edge(
    shape: crate::layout::CellShape,
    origin: Point,
    family: usize,
    near: bool,
) -> (Point, Point) {
    match shape {
        crate::layout::CellShape::Square => shape.family_edge(origin, 1 - family, near),
        _ => {
            let others: Vec<usize> = (0..3).filter(|f| *f != family).collect();
            let edge_family = if shape.triangle_edge_is_near(others[0]) == near {
                others[0]
            } else {
                others[1]
            };
            shape.family_edge(origin, edge_family, true)
        }
    }
}

/// Where a border sits, as the two ends of the cell edge it runs along. `None` if the border
/// doesn't exist on this grid — which the drawing path relies on, so that annotations left over
/// from a differently-shaped picture are skipped rather than panicking.
fn border_edge(picture: &DynSolution, border: Border) -> Option<(Point, Point)> {
    let lane = picture.lane_map().lanes().get(border.lane)?;
    // Every border but the last is the near side of the cell it precedes; the last one is the far
    // side of the cell it follows.
    let (cell, near) = if border.index < lane.cells.len() {
        (lane.cells[border.index], true)
    } else if border.index == lane.cells.len() {
        (*lane.cells.last()?, false)
    } else {
        return None;
    };
    Some(lane_step_edge(
        picture.cell_shape(cell),
        picture.cell_origin(cell),
        lane.family,
        near,
    ))
}

fn midpoint((a, b): (Point, Point)) -> Point {
    Point::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0)
}

/// The point on a tick's edge that sits `WAVE_INSET` to the `right` of the lane's centre line —
/// where the wave running along that lane should meet it.
///
/// The inset is measured *across* the lane, but a tick lies along its own cell edge, and those
/// two directions only coincide where the edge is square to the lane. On a square grid it always
/// is; on a triddler a row's ticks are slanted 30° off, so stepping `WAVE_INSET` straight across
/// the lane from the edge's midpoint lands beside the tick rather than on it, and the wave stops
/// short. Slide along the edge instead, by however far it takes to gain `WAVE_INSET` across.
fn inset_along((a, b): (Point, Point), right: crate::layout::Vec2) -> Point {
    let mid = midpoint((a, b));
    let edge = crate::layout::Vec2::new(b.x - a.x, b.y - a.y);
    // How much of the edge's length is gained across the lane. A border always crosses its lane,
    // so this is never zero; guard it anyway rather than dividing by it blind.
    let across = edge.x * right.x + edge.y * right.y;
    if across.abs() <= f32::EPSILON {
        return Point::new(mid.x + right.x * WAVE_INSET, mid.y + right.y * WAVE_INSET);
    }
    // Never past the edge's own end, however wide the inset is set.
    let t = (WAVE_INSET / across).clamp(-0.5, 0.5);
    Point::new(mid.x + edge.x * t, mid.y + edge.y * t)
}

/// How far the pointer must travel, as a fraction of a cell, before a press counts as a drag
/// rather than a click.
///
/// egui's own click/drag threshold is consulted too, but it can't carry this alone: it also
/// promotes a *slow* press to a drag once `max_click_duration` is up, so holding still for a
/// moment before letting go would leave a stray one-cell mark instead of clearing the cell.
const DRAG_MINIMUM: f32 = 0.2;

/// The mark a drag of `drag` away from `origin` describes.
///
/// Anchoring on a cell rather than on a border is what lets a mark rotate: every lane through the
/// cell is a candidate, and `tools::lane_along_drag` — which the line tool asks the same question
/// of — picks among them afresh on every frame of the drag.
fn span_from_drag(
    picture: &DynSolution,
    origin: u32,
    drag: crate::layout::Vec2,
) -> Option<Annotation> {
    let along = super::tools::lane_along_drag(picture, origin, drag)?;
    let target = along.target(picture.lane_map().lane(along.lane).cells.len());

    // Border `i` is the near side of cell `i`, so enclosing a run means taking the border before
    // its first cell and the one after its last. Which of the two is the origin's depends on
    // which way the drag ran — that's what makes the mark rotate about the origin cell rather
    // than pivot on one of its edges, and it holds even for a nudge that never leaves that cell.
    let (from, to) = if along.forward() {
        (along.from, target + 1)
    } else {
        (along.from + 1, target)
    };
    Some(Annotation {
        lane: along.lane,
        from,
        to,
    })
}

impl CanvasGui {
    /// Drive the annotate tool from the pointer. Takes an abstract-unit position rather than a
    /// cell, so that the mark answers to where the pointer actually is rather than snapping
    /// between cell centres.
    ///
    /// Deliberately calls neither `perform` nor anything that bumps `version`: annotations are
    /// scratch, invisible both to undo and to every cache `version` guards.
    pub(super) fn annotate_input(&mut self, pointer: AnnotatePointer, p: Point) {
        let Some(picture) = self.document.try_solution() else {
            return;
        };

        if pointer.pressed {
            self.annotate_drag = picture
                .cell_at(p)
                .and_then(|coord| picture.cell_of(coord))
                .map(|origin| AnnotateDrag {
                    origin,
                    press: p,
                    live: None,
                });
            // Press and release usually land in separate frames, but a fast enough click — or a
            // slow enough frame — delivers both at once. Fall through and let that settle as the
            // click it is, rather than dropping it.
            if !pointer.released {
                return;
            }
        }

        let Some(drag) = &self.annotate_drag else {
            return;
        };
        let origin = drag.origin;
        let motion = crate::layout::Vec2::new(p.x - drag.press.x, p.y - drag.press.y);
        let far_enough =
            pointer.dragging && (motion.x * motion.x + motion.y * motion.y).sqrt() >= DRAG_MINIMUM;
        // Once a drag is under way it keeps its last mark, so wandering back to exactly where the
        // press landed doesn't make the mark vanish mid-drag.
        let live = far_enough
            .then(|| span_from_drag(picture, origin, motion))
            .flatten()
            .or(drag.live);

        if pointer.released {
            match live {
                // Drags are purely additive; clearing is the click's job alone.
                Some(annotation) => self.annotations.push(annotation),
                None => {
                    // A click clears every mark covering the cell — and if there was nothing to
                    // clear, marks that single cell instead. Dragging out a one-cell mark means
                    // keeping the pointer inside one cell, which is fiddly enough that the click
                    // is the way to reach the commonest count of all.
                    let before = self.annotations.len();
                    self.annotations.retain(|a| !a.covers(picture, origin));
                    if self.annotations.len() == before
                        && let Some(mark) = Annotation::lone(picture, origin)
                    {
                        self.annotations.push(mark);
                    }
                }
            }
            self.annotate_drag = None;
        } else if pointer.down {
            self.annotate_drag.as_mut().unwrap().live = live;
        }
    }

    /// Paint the annotations, including whichever one is being dragged out right now.
    ///
    /// Called after the picture's own shapes have gone to the painter, so these land on top of
    /// the cells, the grid guides and the lasso's ants.
    pub(super) fn draw_annotations(
        &self,
        ui: &egui::Ui,
        painter: &egui::Painter,
        scale: f32,
        to_screen: &egui::emath::RectTransform,
    ) {
        let Some(picture) = self.document.try_solution() else {
            return;
        };

        let live = self.annotate_drag.as_ref().and_then(|drag| drag.live);
        let mut marks = Marks::default();
        for annotation in self.annotations.iter().copied().chain(live) {
            marks.collect(ui, picture, scale, to_screen, annotation);
        }
        marks.paint(painter);
    }
}

/// How far the wave sits from the lane's own centre line, in abstract units. Comfortably inside
/// a lane, even a triangular one (which is only `TRI_ROW_HEIGHT` across).
const WAVE_INSET: f32 = 0.27;
/// How far the wave swings either side of that line.
const WAVE_AMPLITUDE: f32 = 0.07;
/// One full swing of the wave, measured along the lane.
const WAVE_WAVELENGTH: f32 = 0.45;
/// Line segments per swing.
const WAVE_STEPS_PER_CYCLE: usize = 12;
/// A count's size, as a fraction of a cell. A bit smaller than a clue's own label: this shares
/// the cells with the picture.
const LABEL_FONT_SCALE: f32 = 0.6;
/// How far the white halo extends past the black, in screen pixels.
const HALO: f32 = 1.5;

/// How thick a mark's black core is — the same for a border tick as for a wave, so a span reads
/// as one drawn line rather than as three. Floored so it doesn't vanish when zoomed out.
fn mark_width(scale: f32) -> f32 {
    (scale * 0.07).max(1.5)
}

/// The same polyline, run `by` further on at each end.
///
/// epaint gives an open path butt caps, so simply stroking a wider white copy haloes a mark's
/// sides but stops dead level with its ends. Stretching the white copy puts the same margin
/// around the caps as there is along the sides.
fn extended(points: &[Pos2], by: f32) -> Vec<Pos2> {
    let past = |from: Pos2, end: Pos2| -> Pos2 {
        let away = end - from;
        let length = away.length();
        if length <= f32::EPSILON {
            end
        } else {
            end + away / length * by
        }
    };

    let mut out = points.to_vec();
    let last = out.len() - 1;
    out[0] = past(points[1], points[0]);
    out[last] = past(points[last - 1], points[last]);
    out
}

fn screen_edge((a, b): (Point, Point), to_screen: &egui::emath::RectTransform) -> [Pos2; 2] {
    [
        to_screen * Pos2::new(a.x, a.y),
        to_screen * Pos2::new(b.x, b.y),
    ]
}

/// A count written over the picture, worked out while the marks are being gathered so that its
/// white outline can go down in the same pass as every other white.
struct Label {
    center: Pos2,
    text: String,
    font: egui::FontId,
    /// How far the outline stamps sit from the text.
    offset: f32,
}

/// Everything to be drawn this frame, gathered before any of it is painted.
///
/// Each mark is black with a white halo under it, so that it reads against a cell of any color —
/// but painted one mark at a time, a halo rubs out whatever black is already beneath it. That
/// happens between annotations wherever two of them cross, and within a single one as well: a
/// span's end ticks would erase the ends of its own wave. So all the white goes down first, and
/// all the black on top of it — and then the counts on top of everything, since a number has to
/// stay readable however crowded the marks around it get.
#[derive(Default)]
struct Marks {
    lines: Vec<MarkLine>,
    labels: Vec<Label>,
}

/// One stroked path, with the thickness of its black core.
struct MarkLine {
    points: Vec<Pos2>,
    width: f32,
    /// A closed loop joins back to its own start, so it has no caps for the halo to run past.
    closed: bool,
}

impl Marks {
    fn line(&mut self, points: Vec<Pos2>, width: f32) {
        self.push(points, width, false);
    }

    /// A path that joins back to its own start, like the box round a single-cell mark.
    fn closed_line(&mut self, points: Vec<Pos2>, width: f32) {
        self.push(points, width, true);
    }

    fn push(&mut self, points: Vec<Pos2>, width: f32, closed: bool) {
        if points.len() >= 2 {
            self.lines.push(MarkLine {
                points,
                width,
                closed,
            });
        }
    }

    /// A cell count, to be written over everything else once the marks are down.
    fn label(&mut self, ui: &egui::Ui, scale: f32, center: Pos2, count: usize) {
        let text = count.to_string();
        self.labels.push(Label {
            font: solver::clue_font(ui, &text, scale, LABEL_FONT_SCALE),
            center,
            text,
            offset: (scale * 0.09).max(1.5),
        });
    }

    /// Gather one annotation: a tick past each end, plus the wave running between them and the
    /// count it carries.
    fn collect(
        &mut self,
        ui: &egui::Ui,
        picture: &DynSolution,
        scale: f32,
        to_screen: &egui::emath::RectTransform,
        annotation: Annotation,
    ) {
        let width = mark_width(scale);

        // A single cell gets a box round it and a bare "1", since there's no run to lay a wave
        // along and no direction to point it in.
        if let Some(cell) = annotation.lone_cell(picture) {
            let shape = picture.cell_shape(cell);
            let origin = picture.cell_origin(cell);
            let (corners, n) = shape.vertices(origin);
            self.closed_line(
                corners[..n]
                    .iter()
                    .map(|corner| to_screen * Pos2::new(corner.x, corner.y))
                    .collect(),
                width,
            );
            let center = shape.center(origin);
            self.label(ui, scale, to_screen * Pos2::new(center.x, center.y), 1);
            return;
        }

        let (start, end) = annotation.ends();
        // A mark left over from a differently-shaped picture simply doesn't draw.
        let (Some(start), Some(end)) = (border_edge(picture, start), border_edge(picture, end))
        else {
            return;
        };

        self.line(screen_edge(start, to_screen).to_vec(), width);
        self.line(screen_edge(end, to_screen).to_vec(), width);
        self.wave(ui, scale, to_screen, start, end, annotation.cells_covered());
    }

    /// The wave marking out a span: a sine from `from` to `to`, pushed `WAVE_INSET` toward the
    /// right-hand side of that direction — which in egui's y-down space is `(dx, dy) -> (-dy, dx)`
    /// — with the cell count riding on top of it at the midpoint.
    fn wave(
        &mut self,
        ui: &egui::Ui,
        scale: f32,
        to_screen: &egui::emath::RectTransform,
        start: (Point, Point),
        end: (Point, Point),
        count: usize,
    ) {
        // The lane's own direction, taken between the two ticks' midpoints, is what "the
        // right-hand side" is measured against.
        let (from, to) = (midpoint(start), midpoint(end));
        let along = crate::layout::Vec2::new(to.x - from.x, to.y - from.y);
        let length = (along.x * along.x + along.y * along.y).sqrt();
        if length <= f32::EPSILON {
            return;
        }
        let right = crate::layout::Vec2::new(-along.y / length, along.x / length);

        // Where the wave meets each tick, found *on that tick's own edge* rather than by stepping
        // across the lane from its midpoint and hoping to land on it.
        let (head, tail) = (inset_along(start, right), inset_along(end, right));
        let span = crate::layout::Vec2::new(tail.x - head.x, tail.y - head.y);
        let length = (span.x * span.x + span.y * span.y).sqrt();
        if length <= f32::EPSILON {
            return;
        }
        let swing_dir = crate::layout::Vec2::new(-span.y / length, span.x / length);

        // `t` runs 0..1 from one tick to the other; `swing` is the sine's offset across it, in
        // abstract units. The inset is already baked into `head` and `tail`.
        let at = |t: f32, swing: f32| -> Pos2 {
            to_screen
                * Pos2::new(
                    head.x + span.x * t + swing_dir.x * swing,
                    head.y + span.y * t + swing_dir.y * swing,
                )
        };

        let cycles = (length / WAVE_WAVELENGTH).round().max(1.0);
        let steps = cycles as usize * WAVE_STEPS_PER_CYCLE;
        let points = (0..=steps)
            .map(|i| {
                let t = i as f32 / steps as f32;
                at(
                    t,
                    WAVE_AMPLITUDE * (t * cycles * std::f32::consts::TAU).sin(),
                )
            })
            .collect();
        self.line(points, mark_width(scale));

        // The count goes at the midpoint, on the wave's own centre line. Nothing is cut out of
        // the wave for it: the number is painted last, and its outline carves its own hole.
        self.label(ui, scale, at(0.5, 0.0), count);
    }

    /// Every white halo, then every black core on top of it, then the counts above both — each
    /// count outlined the same way, its white stamped in all eight directions, which is what
    /// knocks a hole for it in the wave running underneath.
    ///
    /// (`solver::draw_bare_number` won't do for those: it decides whether an outline is needed by
    /// contrast against the *panel*, and black on the panel looks perfectly legible right up
    /// until the number lands on a black cell.)
    fn paint(&self, painter: &egui::Painter) {
        for line in &self.lines {
            let stroke = egui::Stroke::new(line.width + 2.0 * HALO, Color32::WHITE);
            painter.add(if line.closed {
                egui::Shape::closed_line(line.points.clone(), stroke)
            } else {
                egui::Shape::line(extended(&line.points, HALO), stroke)
            });
        }
        for line in &self.lines {
            let stroke = egui::Stroke::new(line.width, Color32::BLACK);
            painter.add(if line.closed {
                egui::Shape::closed_line(line.points.clone(), stroke)
            } else {
                egui::Shape::line(line.points.clone(), stroke)
            });
        }

        // The same white-before-black split again, so that two counts crowding each other behave
        // the way two crossing marks do.
        for label in &self.labels {
            for dx in [-1.0, 0.0, 1.0] {
                for dy in [-1.0, 0.0, 1.0] {
                    if (dx, dy) == (0.0, 0.0) {
                        continue;
                    }
                    painter.text(
                        label.center + Vec2::new(dx * label.offset, dy * label.offset),
                        egui::Align2::CENTER_CENTER,
                        &label.text,
                        label.font.clone(),
                        Color32::WHITE,
                    );
                }
            }
        }
        for label in &self.labels {
            painter.text(
                label.center,
                egui::Align2::CENTER_CENTER,
                &label.text,
                label.font.clone(),
                Color32::BLACK,
            );
        }
    }
}

#[cfg(test)]
mod annotate_tests {
    use super::*;
    use crate::puzzle::Solution;

    /// A 6x6 blank picture with the annotate tool selected. Cell `(x, y)` is `y * 6 + x`; row `y`
    /// is lane `y` and column `x` is lane `6 + x` (see `Square::build`).
    fn canvas() -> CanvasGui {
        let mut gui = NonogramGui::new(Document::from_solution(
            DynSolution::Square(Solution::blank_bw(6, 6)),
            "test".to_string(),
        ))
        .editor_gui;
        gui.current_tool = Tool::Annotate;
        gui.allow_annotations = true;
        gui
    }

    /// The middle of cell `(x, y)`, which is where a press is aimed: a mark is anchored on the
    /// cell it started in, so anywhere inside will do.
    fn cell(x: f32, y: f32) -> (f32, f32) {
        (x + 0.5, y + 0.5)
    }

    fn at(gui: &mut CanvasGui, phase: AnnotatePointer, (x, y): (f32, f32)) {
        gui.annotate_input(phase, Point::new(x, y));
    }

    const PRESS: AnnotatePointer = AnnotatePointer {
        pressed: true,
        down: true,
        released: false,
        dragging: false,
    };
    /// Moving and releasing once egui has decided this is a drag rather than a click.
    const MOVE: AnnotatePointer = AnnotatePointer {
        pressed: false,
        down: true,
        released: false,
        dragging: true,
    };
    const DROP: AnnotatePointer = AnnotatePointer {
        pressed: false,
        down: false,
        released: true,
        dragging: true,
    };
    /// Releasing without egui ever calling it a drag: a click.
    const RELEASE: AnnotatePointer = AnnotatePointer {
        pressed: false,
        down: false,
        released: true,
        dragging: false,
    };

    fn click(gui: &mut CanvasGui, spot: (f32, f32)) {
        at(gui, PRESS, spot);
        at(gui, RELEASE, spot);
    }

    fn drag(gui: &mut CanvasGui, from: (f32, f32), to: (f32, f32)) {
        at(gui, PRESS, from);
        at(gui, MOVE, to);
        at(gui, DROP, to);
    }

    /// A mark covers every cell from the one the drag started in to the one it ended in, and its
    /// two ticks sit on the borders enclosing that run.
    #[test]
    fn a_drag_covers_the_cells_it_ran_over() {
        let mut gui = canvas();

        drag(&mut gui, cell(1.0, 3.0), cell(4.0, 3.0));
        assert_eq!(
            gui.annotations,
            vec![Annotation {
                lane: 3,
                from: 1,
                to: 5
            }]
        );
        assert_eq!(gui.annotations[0].cells_covered(), 4);
    }

    /// The whole point of anchoring on a cell rather than a border: from one origin, a drag can
    /// set off in any direction the grid offers — four on a square grid — and the origin's own
    /// tick swaps to the far side each time, so the run always encloses the origin cell.
    #[test]
    fn a_drag_rotates_around_its_origin() {
        let origin = cell(3.0, 3.0);
        // (where the drag went, which lane it should land on, the two borders it should enclose)
        let cases = [
            (cell(5.0, 3.0), 3, (3, 6)),     // right, along row 3
            (cell(1.0, 3.0), 3, (4, 1)),     // left, same row, origin's tick flipped
            (cell(3.0, 5.0), 6 + 3, (3, 6)), // down, along column 3
            (cell(3.0, 1.0), 6 + 3, (4, 1)), // up, same column
        ];

        for (target, lane, (from, to)) in cases {
            let mut gui = canvas();
            drag(&mut gui, origin, target);
            assert_eq!(
                gui.annotations,
                vec![Annotation { lane, from, to }],
                "dragging from {origin:?} to {target:?}"
            );
            // Whichever way it ran, it covers the origin cell and the two beyond it.
            assert_eq!(gui.annotations[0].cells_covered(), 3);
            assert!(gui.annotations[0].covers(gui.document.try_solution().unwrap(), 3 * 6 + 3));
        }
    }

    /// A nudge that never leaves the origin cell still marks it — one cell, pointing whichever
    /// way the nudge went. Without that, the commonest clue of all would be unmarkable, since a
    /// click is spoken for.
    #[test]
    fn a_nudge_inside_one_cell_marks_that_cell() {
        let origin = cell(3.0, 3.0);

        let mut rightward = canvas();
        drag(&mut rightward, origin, (origin.0 + 0.3, origin.1));
        assert_eq!(
            rightward.annotations,
            vec![Annotation {
                lane: 3,
                from: 3,
                to: 4
            }]
        );
        assert_eq!(rightward.annotations[0].cells_covered(), 1);

        // The same nudge the other way is the same single cell, but running the other way — so
        // its wave lands on the opposite side. This is what a cell-position pair couldn't say.
        let mut leftward = canvas();
        drag(&mut leftward, origin, (origin.0 - 0.3, origin.1));
        assert_eq!(
            leftward.annotations,
            vec![Annotation {
                lane: 3,
                from: 4,
                to: 3
            }]
        );
        assert_eq!(leftward.annotations[0].cells_covered(), 1);
    }

    /// A click on an unmarked cell marks that one cell — the one count a drag can't comfortably
    /// reach, since staying inside a single cell is fiddly. Clicking it again takes it away.
    #[test]
    fn clicking_an_empty_cell_marks_it() {
        let mut gui = canvas();

        click(&mut gui, cell(3.0, 3.0));
        assert_eq!(gui.annotations.len(), 1);
        assert_eq!(gui.annotations[0].cells_covered(), 1);
        assert!(gui.annotations[0].covers(gui.document.try_solution().unwrap(), 3 * 6 + 3));

        click(&mut gui, cell(3.0, 3.0));
        assert!(gui.annotations.is_empty());
    }

    /// Press and release usually arrive in separate frames, but both can land in one — a fast
    /// click, or a slow frame. That still has to register as a click rather than vanishing.
    #[test]
    fn a_press_and_release_in_one_frame_is_still_a_click() {
        let mut gui = canvas();

        at(
            &mut gui,
            AnnotatePointer {
                pressed: true,
                down: false,
                released: true,
                dragging: false,
            },
            cell(3.0, 3.0),
        );
        assert_eq!(gui.annotations.len(), 1);
        assert_eq!(gui.annotations[0].cells_covered(), 1);
    }

    /// A single-cell mark is drawn as a box round its cell rather than as a run, so it needs to
    /// name that cell whichever lane it happened to be filed under — and a run of two or more
    /// never claims to be one.
    #[test]
    fn a_single_cell_mark_knows_its_cell() {
        let mut gui = canvas();
        let picture = gui.document.try_solution().unwrap().clone();

        click(&mut gui, cell(2.0, 4.0));
        assert_eq!(gui.annotations[0].lone_cell(&picture), Some(4 * 6 + 2));

        gui.annotations.clear();
        drag(&mut gui, cell(2.0, 4.0), cell(3.0, 4.0));
        assert_eq!(gui.annotations[0].cells_covered(), 2);
        assert_eq!(gui.annotations[0].lone_cell(&picture), None);
    }

    /// Clicking destroys every mark covering that cell — anywhere along it, not just at an end —
    /// and leaves marks that merely pass nearby alone.
    #[test]
    fn a_click_destroys_what_covers_the_cell() {
        let mut gui = canvas();

        drag(&mut gui, cell(1.0, 3.0), cell(4.0, 3.0)); // row 3, cells 1..=4
        drag(&mut gui, cell(3.0, 1.0), cell(3.0, 4.0)); // column 3, cells 1..=4
        drag(&mut gui, cell(1.0, 5.0), cell(4.0, 5.0)); // row 5, nowhere near
        assert_eq!(gui.annotations.len(), 3);

        // Cell (3, 3) is in the middle of both the row mark and the column mark. Clearing takes
        // priority over marking, so this destroys two and creates nothing.
        click(&mut gui, cell(3.0, 3.0));
        assert_eq!(gui.annotations.len(), 1);
        assert_eq!(gui.annotations[0].lane, 5);

        click(&mut gui, cell(2.0, 5.0));
        assert!(gui.annotations.is_empty());
    }

    /// A cell just past the end of a mark isn't covered by it, so clicking there marks that cell
    /// instead of clearing the mark: border 5 is the far end of a run of cells 1..=4.
    #[test]
    fn a_click_past_the_end_of_a_mark_does_not_clear_it() {
        let mut gui = canvas();

        drag(&mut gui, cell(1.0, 3.0), cell(4.0, 3.0));
        click(&mut gui, cell(5.0, 3.0));

        assert_eq!(gui.annotations.len(), 2);
        assert_eq!(gui.annotations[0].cells_covered(), 4);
        assert_eq!(gui.annotations[1].cells_covered(), 1);
    }

    /// Drags stack rather than replacing: two marks may cover the same run, so a block can be
    /// picked out inside a longer one. Clearing is the click's job alone.
    #[test]
    fn drags_are_additive() {
        let mut gui = canvas();

        drag(&mut gui, cell(0.0, 3.0), cell(5.0, 3.0));
        drag(&mut gui, cell(1.0, 3.0), cell(2.0, 3.0));
        assert_eq!(gui.annotations.len(), 2);
        assert_eq!(gui.annotations[0].cells_covered(), 6);
        assert_eq!(gui.annotations[1].cells_covered(), 2);
    }

    /// A drag off the end of a lane stops at the last cell rather than running past it.
    #[test]
    fn a_drag_stops_at_the_end_of_the_lane() {
        let mut gui = canvas();

        drag(&mut gui, cell(2.0, 3.0), (40.0, 3.5));
        assert_eq!(
            gui.annotations,
            vec![Annotation {
                lane: 3,
                from: 2,
                to: 6
            }]
        );
        assert_eq!(gui.annotations[0].cells_covered(), 4);
    }

    /// On a triddler, anchoring on a cell means all three lane families are reachable from any
    /// press — which is exactly what the old border anchor couldn't manage, since an edge only
    /// ever divides two of the three. Dragging the length of any lane should mark that whole lane.
    #[test]
    fn a_triddler_drag_reaches_every_family() {
        use crate::geometry::{Geometry, Outline, Tri};
        use crate::puzzle::ClueStyle;

        let geometry = Geometry::<Tri>::new(Outline::hexagon(2));
        let cell_count = geometry.cell_count();
        let sol: Solution<Tri> = Solution::new(
            ClueStyle::Nono,
            HashMap::from([(BACKGROUND, ColorInfo::default_bg())]),
            geometry,
            vec![BACKGROUND; cell_count],
        );
        let lane_map = sol.geometry.lane_map().clone();
        let mut gui = NonogramGui::new(Document::from_solution(
            DynSolution::Tri(sol),
            "test".to_string(),
        ))
        .editor_gui;
        gui.current_tool = Tool::Annotate;
        gui.allow_annotations = true;

        for family in 0..3 {
            for lane_idx in lane_map.family(family) {
                let lane = lane_map.lane(lane_idx);
                if lane.cells.len() < 2 {
                    continue;
                }
                let picture = gui.document.try_solution().unwrap();
                let center = |c: u32| picture.cell_shape(c).center(picture.cell_origin(c));
                let (first, last) = (center(lane.cells[0]), center(*lane.cells.last().unwrap()));

                gui.annotations.clear();
                drag(&mut gui, (first.x, first.y), (last.x, last.y));

                let got = gui.annotations[0];
                assert_eq!(
                    got.lane, lane_idx,
                    "a drag along family {family} lane {lane_idx} landed on lane {} instead",
                    got.lane
                );
                assert_eq!(
                    got.cells_covered(),
                    lane.cells.len(),
                    "family {family} lane {lane_idx} came out measuring {} cells",
                    got.cells_covered()
                );
            }
        }
    }

    /// The wave has to meet each of its ticks *on* that tick, not beside it.
    ///
    /// Its inset is measured across the lane, but a tick lies along its own cell edge. On a
    /// square grid those two directions coincide and stepping straight across the lane lands on
    /// the edge by luck; a triddler's row ticks are slanted 30° off, so the same step lands
    /// beside the tick and the wave stops short of it.
    #[test]
    fn the_wave_meets_its_ticks_on_their_own_edges() {
        use crate::geometry::{Geometry, Outline, Tri};
        use crate::puzzle::ClueStyle;

        let square = DynSolution::Square(Solution::blank_bw(6, 5));
        let geometry = Geometry::<Tri>::new(Outline::hexagon(2));
        let cell_count = geometry.cell_count();
        let tri = DynSolution::Tri(Solution::new(
            ClueStyle::Nono,
            HashMap::from([(BACKGROUND, ColorInfo::default_bg())]),
            geometry,
            vec![BACKGROUND; cell_count],
        ));

        for (shape, picture) in [("square", &square), ("triddler", &tri)] {
            let lanes = picture.lane_map();
            for (lane_idx, lane) in lanes.lanes().iter().enumerate() {
                if lane.cells.len() < 2 {
                    continue;
                }
                // A mark running the whole lane, and the same one run backwards — the inset
                // switches sides with the direction, so both are worth checking.
                for (from, to) in [(0, lane.cells.len()), (lane.cells.len(), 0)] {
                    let annotation = Annotation {
                        lane: lane_idx,
                        from,
                        to,
                    };
                    let (start, end) = annotation.ends();
                    let start = border_edge(picture, start).unwrap();
                    let end = border_edge(picture, end).unwrap();

                    let (a, b) = (midpoint(start), midpoint(end));
                    let along = crate::layout::Vec2::new(b.x - a.x, b.y - a.y);
                    let length = (along.x * along.x + along.y * along.y).sqrt();
                    let right = crate::layout::Vec2::new(-along.y / length, along.x / length);

                    for edge in [start, end] {
                        let point = inset_along(edge, right);
                        let (e0, e1) = edge;

                        // On the tick: `point` is a convex combination of the edge's two ends.
                        let edge_vec = (e1.x - e0.x, e1.y - e0.y);
                        let to_point = (point.x - e0.x, point.y - e0.y);
                        let cross = edge_vec.0 * to_point.1 - edge_vec.1 * to_point.0;
                        let t = (to_point.0 * edge_vec.0 + to_point.1 * edge_vec.1)
                            / (edge_vec.0 * edge_vec.0 + edge_vec.1 * edge_vec.1);
                        assert!(
                            cross.abs() < 1e-4,
                            "{shape} lane {lane_idx} ({from} -> {to}): the wave meets the tick {cross} off it"
                        );
                        assert!(
                            (0.0..=1.0).contains(&t),
                            "{shape} lane {lane_idx} ({from} -> {to}): the wave meets the tick past its end (t = {t})"
                        );

                        // ...and at the inset the wave is actually drawn at, measured across the
                        // lane rather than along the edge.
                        let mid = midpoint(edge);
                        let across = (point.x - mid.x) * right.x + (point.y - mid.y) * right.y;
                        assert!(
                            (across - WAVE_INSET).abs() < 1e-4,
                            "{shape} lane {lane_idx} ({from} -> {to}): inset came out {across}, wanted {WAVE_INSET}"
                        );
                    }
                }
            }
        }
    }

    /// The border between two cells of a lane has to be the edge those two cells actually share:
    /// the far side of the earlier one and the near side of the later one must come out as the
    /// same segment. That's what `lane_step_edge` is for, and on a triangular grid — where which
    /// of a cell's three edges leads along a given lane depends on whether it's a ▲ or a ▼ —
    /// getting it wrong would put ticks on edges that aren't in the lane at all.
    #[test]
    fn a_lane_border_is_the_edge_its_two_cells_share() {
        use crate::geometry::{Geometry, Outline, Tri};
        use crate::puzzle::ClueStyle;

        let square = DynSolution::Square(Solution::blank_bw(6, 5));
        let geometry = Geometry::<Tri>::new(Outline::hexagon(2));
        let cell_count = geometry.cell_count();
        let tri = DynSolution::Tri(Solution::new(
            ClueStyle::Nono,
            HashMap::from([(BACKGROUND, ColorInfo::default_bg())]),
            geometry,
            vec![BACKGROUND; cell_count],
        ));

        for picture in [&square, &tri] {
            let lanes = picture.lane_map();
            for (lane_idx, lane) in lanes.lanes().iter().enumerate() {
                for (position, pair) in lane.cells.windows(2).enumerate() {
                    let far_side_of_earlier = lane_step_edge(
                        picture.cell_shape(pair[0]),
                        picture.cell_origin(pair[0]),
                        lane.family,
                        false,
                    );
                    let border = Border {
                        lane: lane_idx,
                        index: position + 1,
                    };
                    let near_side_of_later = border_edge(picture, border).unwrap();

                    // Same segment, but each cell names its own edge in its own winding order.
                    let ends = |(a, b): (Point, Point)| {
                        let mut v = [(a.x, a.y), (b.x, b.y)];
                        v.sort_by(|p, q| p.partial_cmp(q).unwrap());
                        v
                    };
                    let (want, got) = (ends(far_side_of_earlier), ends(near_side_of_later));
                    for (w, g) in want.iter().zip(got.iter()) {
                        assert!(
                            (w.0 - g.0).abs() < 1e-4 && (w.1 - g.1).abs() < 1e-4,
                            "lane {lane_idx} border {}: cells {} and {} disagree, {want:?} vs {got:?}",
                            position + 1,
                            pair[0],
                            pair[1],
                        );
                    }

                    // And every cell edge is one unit long, whatever the shape.
                    let (a, b) = near_side_of_later;
                    let length = ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt();
                    assert!(
                        (length - 1.0).abs() < 1e-4,
                        "lane {lane_idx} border {} came out {length} long",
                        position + 1,
                    );
                }
            }
        }
    }

    /// Annotations are scratch: they never reach the undo system, and nothing they do invalidates
    /// the caches `version` guards (the line analysis, the solved mask, the solve replay).
    #[test]
    fn annotating_is_invisible_to_undo() {
        let mut gui = canvas();

        gui.perform(
            Action::ChangeColor {
                changes: [(7, Color(1))].into(),
            },
            ActionMood::Normal,
        );
        let (version, undos) = (gui.version, gui.undo_stack.len());

        drag(&mut gui, cell(1.0, 3.0), cell(4.0, 3.0));
        click(&mut gui, cell(2.0, 0.0));

        assert_eq!(gui.annotations.len(), 2);
        assert_eq!(gui.version, version);
        assert_eq!(gui.undo_stack.len(), undos);
        assert!(gui.redo_stack.is_empty());

        // And undoing the paint leaves them alone.
        gui.un_or_re_do(true);
        assert_eq!(gui.annotations.len(), 2);
    }
}
