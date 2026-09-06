//! Drawing the picture and handling pointer input over it.
//!
//! Shape-specific work happens in exactly two places: the hit test, and the render loop.
//! Everything else — the tools, undo, the overlays — works in dense cell indices and is the same
//! for every shape. See "triano.rs" and "triddler.rs" for their rendering details.

use super::annotate::AnnotatePointer;
use super::selection::{LassoPointer, marching_ants, selection_outline};
use super::*;

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

/// A clue in a gutter: which lane it belongs to, and which of that lane's clues it is.
pub type ClueId = (usize, usize);

/// What a canvas needs in order to draw clue gutters around the picture. Only the solve view
/// supplies this; the editor draws the picture alone.
pub struct ClueOverlay<'a> {
    pub puzzle: &'a crate::puzzle::DynPuzzle,
    /// One `Vec<LineStatus>` per clue family, in family order.
    pub analysis: Option<&'a Vec<Vec<crate::solve::grid_solve::LineStatus>>>,
    /// One `Vec<usize>` of resolved clue indices per lane, grouped by family like `analysis`.
    pub fixed: Option<&'a Vec<Vec<Vec<usize>>>>,
    pub is_stale: bool,
    /// The hovered cell's block lengths, shown in place of the analysis marks on its own lanes.
    pub hover: Option<HoverBlocks>,
}

impl ClueOverlay<'_> {
    /// Whether the solver has worked a clue out for itself. Those ignore clicks: there's nothing
    /// for the user to check off, and unchecking it would only last until the next repaint.
    pub(super) fn auto_fixed(&self, picture: &DynSolution, (lane, clue_idx): ClueId) -> bool {
        let family = picture.lane_map().lanes()[lane].family;
        let line = lane - picture.lane_map().family(family).start;
        self.fixed
            .and_then(|f| f.get(family)?.get(line))
            .is_some_and(|fixed| fixed.contains(&clue_idx))
    }
}

impl CanvasGui {
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
        self.canvas_with_clues(ui, scale, render_style, None).0
    }

    /// As `canvas`, but growing the drawing area to cover wherever the clues reach, and handing
    /// the gutters themselves to `triddler::draw_clue_gutters`. Returns the hovered cell and whichever clue
    /// box the pointer clicked, if any.
    pub fn canvas_with_clues(
        &mut self,
        ui: &mut egui::Ui,
        scale: f32,
        render_style: RenderStyle,
        clues: Option<ClueOverlay<'_>>,
    ) -> (Option<u32>, Option<ClueId>) {
        let extent = self.document.solution_mut().extent();

        // Grow the drawing area to cover wherever the clues reach — the same bounds the HTML
        // export sizes its drawing to.
        let (lo, hi) = match &clues {
            Some(overlay) => overlay.puzzle.drawing_bounds(),
            None => (
                crate::layout::Point::new(0.0, 0.0),
                crate::layout::Point::new(extent.x, extent.y),
            ),
        };
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

        // Shift is a momentary switch to the annotate tool and a drag holds on to whichever tool
        // started it, so nothing below this line may consult `current_tool` directly.
        let tool = self.effective_tool(ui);

        // While there's room to pan, a middle-drag belongs to the scroll area (see `main_ui`),
        // so it mustn't also reach whatever tool happens to be selected.
        let panning = self.middle_pans && ui.input(|i| i.pointer.middle_down());

        if tool == Tool::Lasso {
            // The lasso is the one tool that must keep tracking the pointer once it leaves the
            // grid — a loop drawn around the outside of a shape is perfectly ordinary — so it
            // works from the abstract-unit position directly, not from a cell.
            if !panning && let Some(pointer_pos) = response.interact_pointer_pos() {
                let p = from_screen * pointer_pos;
                let pointer = LassoPointer::from_egui(&ui.input(|i| i.pointer.clone()));
                self.lasso_input(pointer, Point::new(p.x, p.y));
            }
            self.lasso_keys(ui);
            self.lasso_cursor(ui, hovered_cell);
        } else if tool == Tool::Annotate {
            // A mark is anchored on the cell the drag started from, but it swings around that
            // cell as the pointer moves — so, like the lasso, this wants the abstract-unit
            // position and not just a cell index.
            if !panning && let Some(pointer_pos) = response.interact_pointer_pos() {
                let p = from_screen * pointer_pos;
                let pointer = AnnotatePointer::from_egui(&ui.input(|i| i.pointer.clone()));
                self.annotate_input(pointer, Point::new(p.x, p.y));
            }
            if hovered_cell.is_some() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            }
        } else if hovered_cell.is_some() {
            // There's no brush or paint-bucket in the standard cursor set, so the best these can
            // do is say how precise the tool is: the two that paint a cell the pointer is exactly
            // on get a crosshair, and flood fill — which acts on a whole region — gets the
            // blockier `Cell` instead, just so it doesn't look identical to them.
            ui.ctx().set_cursor_icon(match tool {
                Tool::Pencil | Tool::LineAlongLane => egui::CursorIcon::Crosshair,
                Tool::FloodFill => egui::CursorIcon::Cell,
                Tool::Lasso | Tool::Annotate => unreachable!("handled above"),
            });
        }

        // The lasso and the annotate tool are handled above, where the pointer is still allowed
        // to be somewhere other than on a cell.
        if !panning
            && let Some(pointer_pos) = response.interact_pointer_pos()
            && !matches!(tool, Tool::Lasso | Tool::Annotate)
            && let Some(cell) = cell_under(self.document.solution_mut(), pointer_pos)
        {
            let pointer = ui.input(|i| i.pointer.clone());
            self.pointer_tool_input(cell, &pointer);
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
        //
        // Their boxes are clickable, except when the lasso or the annotate tool is in hand: those
        // two work outside the grid as well as on it, so the clicks that land out here are
        // theirs.
        let mut clicked_clue = None;
        if let Some(overlay) = &clues {
            if !matches!(tool, Tool::Lasso | Tool::Annotate)
                && let Some(pointer_pos) = response.hover_pos()
            {
                let p = from_screen * pointer_pos;
                if let Some(clue) = triddler::clue_box_at(picture, overlay, Point::new(p.x, p.y)) {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    clicked_clue = response.clicked().then_some(clue);
                }
            }
            triddler::draw_clue_gutters(
                ui,
                &painter,
                picture,
                overlay,
                &self.checked_clues,
                scale,
                &to_screen,
            );
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

        // After the picture's own shapes, so the marks land on top of the cells, the grid guides
        // and the lasso's ants.
        self.draw_annotations(ui, &painter, scale, &to_screen);

        response.mark_changed();

        (hovered_cell, clicked_clue)
    }
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
        Some(corner) => triano::half_cell(corner, color, to_screen, screen(origin)),
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
