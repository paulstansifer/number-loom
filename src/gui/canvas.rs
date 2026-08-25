//! Drawing the picture and handling pointer input over it.
//!
//! Shape-specific work happens in exactly two places: the hit test, and the render loop.
//! Everything else — the tools, undo, the overlays — works in dense cell indices and is the same
//! for every shape.

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

    /// As `canvas`, but growing the drawing area to cover wherever the clues reach, and handing
    /// the gutters themselves to `draw_clue_gutters`.
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

        // The lasso is handled above, where the pointer is still allowed off the grid.
        if let Some(pointer_pos) = response.interact_pointer_pos()
            && self.current_tool != Tool::Lasso
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
        if let Some(overlay) = &clues {
            draw_clue_gutters(ui, &painter, picture, overlay, scale, &to_screen);
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
}

/// The clue gutters: the numbers ringing the picture, and the indicator strip between them and
/// the grid.
///
/// Clues share the picture's painter and coordinate system rather than living in their own
/// widgets, because a hexagon's three clue blocks are not axis-aligned rectangles and can't be
/// laid out by a grid of separate panels.
fn draw_clue_gutters(
    ui: &egui::Ui,
    painter: &egui::Painter,
    picture: &DynSolution,
    overlay: &ClueOverlay<'_>,
    scale: f32,
    to_screen: &egui::emath::RectTransform,
) {
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
                solver::draw_string_in_rhombus(ui, painter, &points, &text, scale, color_info.rgb);
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
                Some((len, rgb)) => {
                    solver::draw_bare_number(ui, painter, at, &len.to_string(), scale, rgb)
                }
                None => {
                    if let Some(analysis) = overlay.analysis {
                        let family = lane_families[g.lane];
                        let index = g.lane - family_starts[family];
                        if let Some(status) = analysis.get(family).and_then(|f| f.get(index)) {
                            solver::draw_analysis_mark(
                                painter,
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
