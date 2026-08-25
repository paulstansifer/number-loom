//! The annotate tool, to help counting-out lines during a solve.
//!
//! A click ticks the border nearest the pointer; a lane-constrained drag ticks both ends and
//! writes how many cells lie between them. This doesn't affect the puzzle, and is invisible to
//! the undo system.
//!
//! Everything is stated in terms of lanes rather than coordinates, the way `grid_solve` is, so a
//! triddler needs no special handling: the cell count of a span is just how far apart its two
//! border positions are.

use super::*;

/// A boundary within a lane: the gap just before `lane.cells[index]`, with `index == len` meaning
/// the far end. A lane of n cells therefore has n + 1 borders.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Border {
    pub lane: usize,
    pub index: usize,
}

/// One scratch mark. `from == to` is a bare tick on a single border; otherwise the mark spans
/// `|to - from|` cells, and its wave goes on the right-hand side of `from -> to` — so which way
/// the drag ran is worth keeping, and the two ends are *not* sorted.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Annotation {
    lane: usize,
    from: usize,
    to: usize,
}

impl Annotation {
    fn tick(border: Border) -> Annotation {
        Annotation {
            lane: border.lane,
            from: border.index,
            to: border.index,
        }
    }

    fn start(&self) -> Border {
        Border {
            lane: self.lane,
            index: self.from,
        }
    }

    fn end(&self) -> Border {
        Border {
            lane: self.lane,
            index: self.to,
        }
    }

    /// How many cells this mark spans; zero for a bare tick.
    pub fn cells_covered(&self) -> usize {
        self.from.abs_diff(self.to)
    }

    /// Whether either end of this mark sits on the edge whose midpoint is `spot`.
    fn touches(&self, picture: &DynSolution, spot: Point) -> bool {
        [self.start(), self.end()]
            .into_iter()
            .any(|b| border_midpoint(picture, b).is_some_and(|m| same_spot(m, spot)))
    }
}

/// An annotation being dragged out right now.
pub struct AnnotateDrag {
    /// Every `Border` naming the edge the drag started on — one on a square grid, two on a
    /// triddler (see `borders_near`). Which one the drag ends up on is settled by its direction.
    anchors: Vec<Border>,
    /// Where the press landed, in abstract units; the drag is measured from here.
    press: Point,
    /// What would be committed if the pointer were released right now. Drawn as it goes.
    live: Annotation,
}

/// What the annotate tool needs to know about the pointer in a frame. It doesn't care *which*
/// button: annotating never paints, so every button can drive it.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct AnnotatePointer {
    pressed: bool,
    down: bool,
    released: bool,
}

impl AnnotatePointer {
    pub(super) fn from_egui(pointer: &egui::PointerState) -> AnnotatePointer {
        AnnotatePointer {
            pressed: pointer.any_pressed(),
            down: pointer.any_down(),
            released: pointer.any_released(),
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

fn border_midpoint(picture: &DynSolution, border: Border) -> Option<Point> {
    border_edge(picture, border).map(midpoint)
}

/// Whether two edge midpoints are the same spot. Distinct edges are at least a quarter of a cell
/// apart, so the only thing this ever conflates is the two names a triangular grid gives one edge.
fn same_spot(a: Point, b: Point) -> bool {
    (a.x - b.x).abs() < 1e-3 && (a.y - b.y).abs() < 1e-3
}

/// Every `Border` naming the cell edge nearest `p`; empty when `p` isn't over the grid.
///
/// The hovered cell's own borders are ranked by how far `p` is from each edge's *midpoint*. Over
/// a square's four edges that carves the cell up along its diagonals — exactly the snapping
/// wanted — and the same holds around a triangle's centroid.
///
/// A triangular grid names each edge twice: a cell has three edges but three families of two
/// in-lane neighbours each, so every edge divides two lanes from different families. Both names
/// come back, since a bare tick looks identical either way and only a drag has to choose.
pub(super) fn borders_near(picture: &DynSolution, p: Point) -> Vec<Border> {
    let Some(cell) = picture.cell_at(p).and_then(|coord| picture.cell_of(coord)) else {
        return vec![];
    };

    let mut ranked: Vec<(f32, Border, Point)> = vec![];
    for membership in picture.lane_map().memberships(cell) {
        let position = membership.position as usize;
        for index in [position, position + 1] {
            let border = Border {
                lane: membership.lane as usize,
                index,
            };
            let Some(mid) = border_midpoint(picture, border) else {
                continue;
            };
            let (dx, dy) = (mid.x - p.x, mid.y - p.y);
            ranked.push((dx * dx + dy * dy, border, mid));
        }
    }
    ranked.sort_by(|a, b| a.0.total_cmp(&b.0));

    let Some(&(_, _, nearest)) = ranked.first() else {
        return vec![];
    };
    ranked
        .into_iter()
        .filter(|(_, _, mid)| same_spot(*mid, nearest))
        .map(|(_, border, _)| border)
        .collect()
}

/// The mark a drag of `drag` from `anchors` describes: the anchor whose lane best matches the
/// drag's direction, carried along that lane by however many cells the drag covered.
///
/// The lane-picking and the projection are the same trick the line tool uses (see
/// `tools::line_between`): compare the drag against each candidate lane's first-cell-to-last-cell
/// span, which averages out the zigzag a triangular lane's centroids make.
fn span_from_drag(
    picture: &DynSolution,
    anchors: &[Border],
    drag: crate::layout::Vec2,
) -> Option<Annotation> {
    let first_anchor = *anchors.first()?;
    let drag_len = (drag.x * drag.x + drag.y * drag.y).sqrt();
    if drag_len <= f32::EPSILON {
        return Some(Annotation::tick(first_anchor));
    }

    let lanes = picture.lane_map();
    let center = |cell: u32| picture.cell_shape(cell).center(picture.cell_origin(cell));

    // (anchor, |cos angle| to the drag, how many cells along the lane the drag reached)
    let mut best: Option<(Border, f32, f32)> = None;
    for anchor in anchors {
        let lane = lanes.lane(anchor.lane);
        if lane.cells.len() < 2 {
            continue; // No direction to compare against.
        }
        let first = center(lane.cells[0]);
        let last = center(*lane.cells.last().unwrap());
        let span = crate::layout::Vec2::new(last.x - first.x, last.y - first.y);
        let span_len = (span.x * span.x + span.y * span.y).sqrt();
        let avg_spacing = span_len / (lane.cells.len() - 1) as f32;

        let cos_angle = ((span.x * drag.x + span.y * drag.y) / (span_len * drag_len)).abs();
        let steps = (drag.x * span.x + drag.y * span.y) / span_len / avg_spacing;
        if best.is_none_or(|(_, best_cos, _)| cos_angle > best_cos) {
            best = Some((*anchor, cos_angle, steps));
        }
    }

    let Some((anchor, _, steps)) = best else {
        return Some(Annotation::tick(first_anchor)); // No usable lane; leave a bare tick.
    };
    // Borders, not cells: a lane of n cells has borders 0..=n, so this clamps one higher than the
    // line tool's equivalent does.
    let border_count = lanes.lane(anchor.lane).cells.len() as isize;
    let to = (anchor.index as isize + steps.round() as isize).clamp(0, border_count) as usize;
    Some(Annotation {
        lane: anchor.lane,
        from: anchor.index,
        to,
    })
}

impl CanvasGui {
    /// Drive the annotate tool from the pointer. Takes an abstract-unit position rather than a
    /// cell, since "which border is nearest" is a question about a point.
    ///
    /// Deliberately calls neither `perform` nor anything that bumps `version`: annotations are
    /// scratch, invisible both to undo and to every cache `version` guards.
    pub(super) fn annotate_input(&mut self, pointer: AnnotatePointer, p: Point) {
        let Some(picture) = self.document.try_solution() else {
            return;
        };

        if pointer.pressed {
            let anchors = borders_near(picture, p);
            if let Some(&first) = anchors.first() {
                self.annotate_drag = Some(AnnotateDrag {
                    anchors,
                    press: p,
                    live: Annotation::tick(first),
                });
            }
            return;
        }

        let Some(drag) = &self.annotate_drag else {
            return;
        };
        let motion = crate::layout::Vec2::new(p.x - drag.press.x, p.y - drag.press.y);
        let live = span_from_drag(picture, &drag.anchors, motion).unwrap_or(drag.live);

        if pointer.released {
            if live.from == live.to {
                // The pointer never left the border it started on, so this was a click: it
                // removes whatever is already marked here, and only leaves a tick if there was
                // nothing to remove. Matching is by *position*, so a span's endpoint is fair
                // game, and so is a tick a triddler happens to have filed under the other lane.
                let removed = match border_midpoint(picture, live.start()) {
                    Some(spot) => {
                        let before = self.annotations.len();
                        self.annotations.retain(|a| !a.touches(picture, spot));
                        self.annotations.len() != before
                    }
                    None => false,
                };
                if !removed {
                    self.annotations.push(live);
                }
            } else {
                self.annotations.push(live);
            }
            self.annotate_drag = None;
        } else if pointer.down {
            self.annotate_drag.as_mut().unwrap().live = live;
        }
    }

    /// Paint the annotations, plus a ghost of the tick a click would leave right now.
    ///
    /// Called after the picture's own shapes have gone to the painter, so these land on top of
    /// the cells, the grid guides and the lasso's ants.
    pub(super) fn draw_annotations(
        &self,
        ui: &egui::Ui,
        painter: &egui::Painter,
        scale: f32,
        to_screen: &egui::emath::RectTransform,
        preview: Option<Border>,
    ) {
        let Some(picture) = self.document.try_solution() else {
            return;
        };

        // Under the marks themselves: this is a ghost of one, not one.
        if let Some(border) = preview
            && let Some(edge) = border_edge(picture, border)
        {
            painter.add(egui::Shape::line_segment(
                screen_edge(edge, to_screen),
                egui::Stroke::new(mark_width(scale), Color32::from_black_alpha(96)),
            ));
        }

        let live = self.annotate_drag.as_ref().map(|drag| drag.live);
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
    /// Polylines, with the thickness of their black cores.
    lines: Vec<(Vec<Pos2>, f32)>,
    labels: Vec<Label>,
}

impl Marks {
    fn line(&mut self, points: Vec<Pos2>, width: f32) {
        if points.len() >= 2 {
            self.lines.push((points, width));
        }
    }

    /// Gather one annotation: a tick at each end, plus — for a span — the wave running between
    /// them and the count it carries.
    fn collect(
        &mut self,
        ui: &egui::Ui,
        picture: &DynSolution,
        scale: f32,
        to_screen: &egui::emath::RectTransform,
        annotation: Annotation,
    ) {
        // A mark left over from a differently-shaped picture simply doesn't draw.
        let (Some(start), Some(end)) = (
            border_edge(picture, annotation.start()),
            border_edge(picture, annotation.end()),
        ) else {
            return;
        };

        let width = mark_width(scale);
        self.line(screen_edge(start, to_screen).to_vec(), width);
        if annotation.from == annotation.to {
            return; // A bare tick: both ends name the same border.
        }
        self.line(screen_edge(end, to_screen).to_vec(), width);
        self.wave(
            ui,
            scale,
            to_screen,
            midpoint(start),
            midpoint(end),
            annotation.cells_covered(),
        );
    }

    /// The wave marking out a span: a sine from `from` to `to`, pushed `WAVE_INSET` toward the
    /// right-hand side of that direction — which in egui's y-down space is `(dx, dy) -> (-dy, dx)`
    /// — with the cell count riding on top of it at the midpoint.
    fn wave(
        &mut self,
        ui: &egui::Ui,
        scale: f32,
        to_screen: &egui::emath::RectTransform,
        from: Point,
        to: Point,
        count: usize,
    ) {
        let along = crate::layout::Vec2::new(to.x - from.x, to.y - from.y);
        let length = (along.x * along.x + along.y * along.y).sqrt();
        if length <= f32::EPSILON {
            return;
        }
        let right = crate::layout::Vec2::new(-along.y / length, along.x / length);

        // `t` runs 0..1 along the span; `swing` is the sine's offset across it, in abstract units.
        let at = |t: f32, swing: f32| -> Pos2 {
            let offset = WAVE_INSET + swing;
            to_screen
                * Pos2::new(
                    from.x + along.x * t + right.x * offset,
                    from.y + along.y * t + right.y * offset,
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
        let text = count.to_string();
        self.labels.push(Label {
            font: solver::clue_font(ui, &text, scale, LABEL_FONT_SCALE),
            center: at(0.5, 0.0),
            text,
            offset: (scale * 0.09).max(1.5),
        });
    }

    /// Every white halo, then every black core on top of it, then the counts above both — each
    /// count outlined the same way, its white stamped in all eight directions, which is what
    /// knocks a hole for it in the wave running underneath.
    ///
    /// (`solver::draw_bare_number` won't do for those: it decides whether an outline is needed by
    /// contrast against the *panel*, and black on the panel looks perfectly legible right up
    /// until the number lands on a black cell.)
    fn paint(&self, painter: &egui::Painter) {
        for (points, width) in &self.lines {
            painter.add(egui::Shape::line(
                extended(points, HALO),
                egui::Stroke::new(width + 2.0 * HALO, Color32::WHITE),
            ));
        }
        for (points, width) in &self.lines {
            painter.add(egui::Shape::line(
                points.clone(),
                egui::Stroke::new(*width, Color32::BLACK),
            ));
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

    fn at(gui: &mut CanvasGui, phase: AnnotatePointer, x: f32, y: f32) {
        gui.annotate_input(phase, Point::new(x, y));
    }

    const PRESS: AnnotatePointer = AnnotatePointer {
        pressed: true,
        down: true,
        released: false,
    };
    const MOVE: AnnotatePointer = AnnotatePointer {
        pressed: false,
        down: true,
        released: false,
    };
    const RELEASE: AnnotatePointer = AnnotatePointer {
        pressed: false,
        down: false,
        released: true,
    };

    fn click(gui: &mut CanvasGui, x: f32, y: f32) {
        at(gui, PRESS, x, y);
        at(gui, RELEASE, x, y);
    }

    fn drag(gui: &mut CanvasGui, from: (f32, f32), to: (f32, f32)) {
        at(gui, PRESS, from.0, from.1);
        at(gui, MOVE, to.0, to.1);
        at(gui, RELEASE, to.0, to.1);
    }

    /// The border nearest a point is the one whose edge midpoint is closest, which carves each
    /// square cell up along its diagonals: near the right edge of `(2, 3)` that's the *row*'s
    /// border, and near the bottom edge it's the *column*'s.
    #[test]
    fn snapping_picks_the_nearest_edge() {
        let gui = canvas();
        let picture = gui.document.try_solution().unwrap();

        assert_eq!(
            borders_near(picture, Point::new(2.9, 3.5)),
            vec![Border { lane: 3, index: 3 }]
        );
        assert_eq!(
            borders_near(picture, Point::new(2.5, 3.9)),
            vec![Border {
                lane: 6 + 2,
                index: 4
            }]
        );
        // The far side of the last cell in a lane is a border too.
        assert_eq!(
            borders_near(picture, Point::new(5.9, 3.5)),
            vec![Border { lane: 3, index: 6 }]
        );
        assert!(borders_near(picture, Point::new(9.0, 9.0)).is_empty());
    }

    /// Clicking a border marks it; clicking the same border again takes the mark away.
    #[test]
    fn clicking_a_border_toggles_a_tick() {
        let mut gui = canvas();

        click(&mut gui, 2.9, 3.5);
        assert_eq!(
            gui.annotations,
            vec![Annotation {
                lane: 3,
                from: 3,
                to: 3
            }]
        );

        // Somewhere else in the same cell, but still nearest the same edge.
        click(&mut gui, 2.95, 3.4);
        assert!(gui.annotations.is_empty());
    }

    /// A drag spans as many cells as it covered, in the direction it ran — so dragging the same
    /// stretch backwards is a different annotation, and puts the wave on the other side.
    #[test]
    fn dragging_measures_the_cells_it_covers() {
        let mut gui = canvas();

        drag(&mut gui, (0.1, 3.5), (4.1, 3.5));
        assert_eq!(
            gui.annotations,
            vec![Annotation {
                lane: 3,
                from: 0,
                to: 4
            }]
        );
        assert_eq!(gui.annotations[0].cells_covered(), 4);

        gui.annotations.clear();
        drag(&mut gui, (4.1, 3.5), (0.1, 3.5));
        assert_eq!(
            gui.annotations,
            vec![Annotation {
                lane: 3,
                from: 4,
                to: 0
            }]
        );
    }

    /// A drag off the end of a lane stops at the lane's last border rather than running past it.
    /// A lane of six cells has *seven* borders, so the limit is 6, not 5.
    #[test]
    fn a_drag_stops_at_the_end_of_the_lane() {
        let mut gui = canvas();

        drag(&mut gui, (2.1, 3.5), (40.0, 3.5));
        assert_eq!(
            gui.annotations,
            vec![Annotation {
                lane: 3,
                from: 2,
                to: 6
            }]
        );
    }

    /// Clicking either end of a span removes the whole thing — the click is matched by position,
    /// so it doesn't matter that the span wasn't a tick.
    #[test]
    fn clicking_an_end_of_a_span_removes_it() {
        let mut gui = canvas();

        drag(&mut gui, (0.1, 3.5), (4.1, 3.5));
        assert_eq!(gui.annotations.len(), 1);

        click(&mut gui, 3.9, 3.5); // The border at index 4: the span's far end.
        assert!(gui.annotations.is_empty());
    }

    /// On a triangular grid every edge divides two lanes from *different* families, so a click
    /// alone can't say which lane is meant — but a drag can. Dragging along a `/` lane must
    /// annotate that lane, not the row that shares the edge it started on.
    #[test]
    fn a_triddler_drag_picks_the_lane_it_ran_along() {
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

        // Every "/" lane (family 1) long enough to drag along: press just inside the border at
        // one end and release just inside the border at the other, and the whole lane should come
        // out marked. Aiming at the borders rather than at the end cells' centroids matters —
        // from a centroid the nearest edge may well be one that doesn't divide this lane at all.
        for lane_idx in lane_map.family(1) {
            let lane = lane_map.lane(lane_idx);
            if lane.cells.len() < 2 {
                continue;
            }
            let picture = gui.document.try_solution().unwrap();
            // A third of the way from a border toward the cell beside it: unambiguously inside
            // that cell, and unambiguously nearest that border.
            let just_inside = |border: Border, cell: u32| {
                let mid = border_midpoint(picture, border).unwrap();
                let center = picture.cell_shape(cell).center(picture.cell_origin(cell));
                (
                    mid.x + (center.x - mid.x) / 3.0,
                    mid.y + (center.y - mid.y) / 3.0,
                )
            };
            let start = just_inside(
                Border {
                    lane: lane_idx,
                    index: 0,
                },
                lane.cells[0],
            );
            let end = just_inside(
                Border {
                    lane: lane_idx,
                    index: lane.cells.len(),
                },
                *lane.cells.last().unwrap(),
            );

            gui.annotations.clear();
            drag(&mut gui, start, end);

            let got = gui.annotations[0];
            assert_eq!(
                got.lane, lane_idx,
                "a drag along lane {lane_idx} landed on lane {} instead",
                got.lane
            );
            assert_eq!(
                got.cells_covered(),
                lane.cells.len(),
                "lane {lane_idx} (len {}) came out measuring {} cells",
                lane.cells.len(),
                got.cells_covered()
            );
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

        drag(&mut gui, (0.1, 3.5), (4.1, 3.5));
        click(&mut gui, 2.5, 0.1);

        assert_eq!(gui.annotations.len(), 2);
        assert_eq!(gui.version, version);
        assert_eq!(gui.undo_stack.len(), undos);
        assert!(gui.redo_stack.is_empty());

        // And undoing the paint leaves them alone.
        gui.un_or_re_do(true);
        assert_eq!(gui.annotations.len(), 2);
    }
}
