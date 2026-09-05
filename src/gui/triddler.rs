//! Drawing for triangular grids.
//!
//! A triddler's cells are ▲▼ rather than squares, which mostly the geometry takes care of. What
//! it doesn't take care of is the clues: a triddler has three clue families rather than two, and
//! their gutters run off the picture at 60° to each other, so they can't be laid out as panels
//! beside the grid the way `solver::draw_clues` lays out a square puzzle's. They're drawn into
//! the picture's own painter instead (`draw_clue_gutters`), in boxes shaped like rhombuses rather
//! than squares — which is why the numbers in them need their own sizing.
//!
//! The sidebar's hover readout has the same problem, and `draw_rosette` is the answer to it: six
//! rhombus arms around the hovered triangle instead of a square grid's four square ones.
//!
//! A trianogram's half-square "cap" colors are a different thing entirely; see `super::triano`.

use super::canvas::{ClueId, ClueOverlay};
use super::solver::{
    self, draw_bare_number_sized, draw_string_in_polygon, fill_polygon, polygon_centroid,
};
use egui::{Pos2, Vec2};
use std::collections::HashSet;

use crate::layout::Point;
use crate::puzzle::{Clue, ColorInfo, DynSolution};

/// A rhombus clue box's label is smaller than a square one's: the rhombus is narrower
/// top-to-bottom than it is wide, so a square box's font wouldn't fit between its slanted sides.
const RHOMBUS_FONT_SCALE: f32 = 0.5;

/// How much room the sidebar's rosette needs, in units of the cell scale. More than a square
/// grid's, because a rhombus is wider across its short diagonal than it is long, and there are
/// six arms 60° apart rather than four at right angles.
pub(super) const ROSETTE_SIZE: f32 = 4.4;

/// As `solver::draw_string_in_polygon`, but for a rhombus clue box specifically: the label is
/// smaller than a square box's, and nudged up a bit from the geometric middle, since the
/// rhombus's mass sits toward its bottom half.
pub(super) fn draw_string_in_rhombus(
    ui: &egui::Ui,
    painter: &egui::Painter,
    points: &[Pos2],
    clue_txt: &str,
    scale: f32,
    rgb: (u8, u8, u8),
) {
    fill_polygon(painter, points, rgb);
    let center = polygon_centroid(points);
    solver::draw_string_at(
        ui,
        painter,
        center,
        clue_txt,
        scale,
        rgb,
        RHOMBUS_FONT_SCALE,
    );
}

/// `solver::draw_bare_number` for a rhombus clue box: what a resolved clue in a triddler's gutter
/// gets in place of its filled rhombus.
pub(super) fn draw_bare_number_in_rhombus(
    ui: &egui::Ui,
    painter: &egui::Painter,
    points: &[Pos2],
    txt: &str,
    scale: f32,
    rgb: (u8, u8, u8),
) {
    draw_bare_number_sized(
        ui,
        painter,
        polygon_centroid(points),
        txt,
        scale,
        rgb,
        RHOMBUS_FONT_SCALE,
    );
}

/// One lane's clue boxes in the order they're drawn, counting outward from the grid: the color,
/// the count (`None` for a triano cap), and which of the lane's clues the box belongs to.
fn expressed_clues(
    puzzle: &crate::puzzle::DynPuzzle,
    g: &crate::layout::GutterLane,
) -> Vec<(ColorInfo, Option<u16>, usize)> {
    crate::with_puzzle!(puzzle, |p| {
        let mut v: Vec<(ColorInfo, Option<u16>, usize)> = p.lines[g.lane]
            .iter()
            .enumerate()
            .flat_map(|(clue_idx, c)| {
                c.express(&p.palette)
                    .into_iter()
                    .map(move |(ci, n)| (ci.clone(), n, clue_idx))
            })
            .collect();
        // Clues run in the lane's own direction, so the box nearest the grid is the last one;
        // `reversed` covers the families whose clues are labelled at the far end from where the
        // lane is stored.
        if !g.reversed {
            v.reverse();
        }
        v
    })
}

/// The corners of a gutter's `i`th clue box, in abstract units.
fn clue_box_points(g: &crate::layout::GutterLane, family: usize, i: usize) -> [Point; 4] {
    crate::layout::tri_clue_rhombus(
        g.clue_box_center(i),
        family,
        g.edge_dir,
        crate::layout::CLUE_BOX,
        crate::layout::CLUE_BOX_SHORT,
    )
}

/// The clue whose gutter box covers `at` (in abstract units). A box the solver has checked off
/// itself swallows the click rather than reporting it: there's nothing left to check off there.
pub(super) fn clue_box_at(
    picture: &DynSolution,
    overlay: &ClueOverlay<'_>,
    at: Point,
) -> Option<ClueId> {
    for (_, gutter) in picture.gutters() {
        for g in gutter {
            let family = picture.lane_map().lanes()[g.lane].family;
            for (i, (_, _, clue_idx)) in expressed_clues(overlay.puzzle, g).iter().enumerate() {
                if crate::layout::convex_contains(&clue_box_points(g, family, i), at) {
                    let id = (g.lane, *clue_idx);
                    return (!overlay.auto_fixed(picture, id)).then_some(id);
                }
            }
        }
    }
    None
}

/// The clue gutters: the numbers ringing the picture, and the indicator strip between them and
/// the grid.
///
/// Clues share the picture's painter and coordinate system rather than living in their own
/// widgets, because a hexagon's three clue blocks are not axis-aligned rectangles and can't be
/// laid out by a grid of separate panels.
pub(super) fn draw_clue_gutters(
    ui: &egui::Ui,
    painter: &egui::Painter,
    picture: &DynSolution,
    overlay: &ClueOverlay<'_>,
    checked: &HashSet<ClueId>,
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
            // Each entry carries the clue it came from, since a clue can express as several
            // boxes and it's the whole clue that gets resolved.
            let expressed = expressed_clues(overlay.puzzle, g);

            let family = lane_families[g.lane];
            for (i, (color_info, count, clue_idx)) in expressed.iter().enumerate() {
                let points = clue_box_points(g, family, i).map(|p| to_screen * Pos2::new(p.x, p.y));
                let text = match count {
                    Some(n) => n.to_string(),
                    None => color_info.ch.to_string(),
                };
                // A resolved clue loses its box: nothing about it is left to work out.
                let id = (g.lane, *clue_idx);
                if checked.contains(&id) || overlay.auto_fixed(picture, id) {
                    draw_bare_number_in_rhombus(ui, painter, &points, &text, scale, color_info.rgb);
                } else {
                    draw_string_in_rhombus(ui, painter, &points, &text, scale, color_info.rgb);
                }
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

/// The sidebar's hover readout: the hovered cell in the middle, ringed by the block lengths
/// running out of it in each of the six lane directions.
///
/// The centre swatch matches the hovered triangle's own shape (▲ or ▼) instead of a generic
/// square, and the arms are shaped like the rhombus that lane's clue boxes use, so they read as
/// belonging to that lane. They're pushed further out than a square grid's arms so that adjacent
/// rhombuses (60° apart, wide across their short diagonal) don't overlap each other or the centre.
pub(super) fn draw_rosette(
    ui: &egui::Ui,
    painter: &egui::Painter,
    picture: &DynSolution,
    cell: u32,
    center: Pos2,
    scale: f32,
) {
    let color = picture.cells()[cell as usize];
    let rgb = picture.palette()[&color].rgb;
    let text = if color == crate::puzzle::UNSOLVED {
        "?"
    } else {
        " "
    };

    // One run per clue family, three families; `arm_directions` places the two arms of each.
    let runs = picture.runs_at_cell(cell);
    let dirs = picture.arm_directions();

    let cell_shape = picture.cell_shape(cell);
    let mid_size = crate::layout::Vec2::new(scale, scale * crate::layout::TRI_ROW_HEIGHT);
    let (verts, n) = cell_shape.vertices_sized(
        Point::new(center.x - mid_size.x / 2.0, center.y - mid_size.y / 2.0),
        mid_size,
    );
    let mid_points: Vec<Pos2> = verts[..n].iter().map(|p| Pos2::new(p.x, p.y)).collect();
    draw_string_in_polygon(ui, painter, &mid_points, text, scale, rgb);

    let arm_size = scale * 0.68;
    let arm_distance = scale * 1.7;
    for (family, (back, forward)) in runs.iter().enumerate() {
        for (i, count) in [back, forward].into_iter().enumerate() {
            let Some(dir) = dirs.get(family * 2 + i) else {
                continue;
            };
            if *count == 0 {
                continue;
            }
            let arm_center = center + Vec2::new(dir.x, dir.y) * arm_distance;
            // The arm's short side should sit flush against this cell's real edge for the
            // *other* family it borders — "near" (leading to the previous cell) for the back
            // arm, "far" for the forward arm.
            let (ea, eb) =
                super::annotate::lane_step_edge(cell_shape, Point::new(0.0, 0.0), family, i == 0);
            let (edx, edy) = (eb.x - ea.x, eb.y - ea.y);
            let elen = (edx * edx + edy * edy).sqrt();
            let edge_dir = if elen > 0.0 {
                crate::layout::Vec2::new(edx / elen, edy / elen)
            } else {
                crate::layout::Vec2::new(0.0, 1.0)
            };
            // Same long/short proportion as a real clue box, so this preview actually looks like
            // the gutter boxes it's previewing.
            let arm_short = arm_size * (crate::layout::CLUE_BOX_SHORT / crate::layout::CLUE_BOX);
            let points: Vec<Pos2> = crate::layout::tri_clue_rhombus(
                Point::new(arm_center.x, arm_center.y),
                family,
                edge_dir,
                arm_size,
                arm_short,
            )
            .iter()
            .map(|p| Pos2::new(p.x, p.y))
            .collect();
            draw_string_in_rhombus(ui, painter, &points, &count.to_string(), scale, rgb);
        }
    }
}
