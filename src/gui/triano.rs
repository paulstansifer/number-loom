//! Drawing trianogram caps.
//!
//! A trianogram's extra "colors" are half-squares — a cell split along a diagonal — and they only
//! ever show up as the caps on the ends of a clue. So there are two things to draw here: the
//! half-square itself, wherever a `Corner` color lands (a picture cell, a replay frame, a gutter
//! box), and the silhouette a capped clue gets in the gutter, which spans the clue's boxes and
//! their caps as one shape.
//!
//! None of this has anything to do with a triddler's triangular *cells*; that's `super::triddler`.

use super::solver::{clue_outline_width, contrast_outline};
use egui::{Color32, Pos2, Rect, Vec2};

use crate::puzzle::{ColorInfo, Corner};

/// A cap's corners, in a box of `scale` at the origin. `layout::corner_triangle` has the shape;
/// this adds the half-pixel nudges that make the seams between adjacent shapes on screen close up
/// (the `+`ed offsets are empirically set to make things fit better), and converts to egui.
fn triangle_points(corner: Corner, scale: Vec2) -> Vec<Pos2> {
    let (points, n) = crate::layout::corner_triangle(
        corner.upper,
        corner.left,
        crate::layout::Point::new(0.0, 0.0),
        crate::layout::Vec2::new(scale.x, scale.y),
    );
    points[..n]
        .iter()
        .map(|p| {
            let fudge = if p.y > 0.0 { 0.5 } else { -0.5 };
            Pos2::new(p.x + 0.25, p.y + fudge)
        })
        .collect()
}

fn triangle_shape(corner: Corner, color: Color32, scale: Vec2) -> egui::Shape {
    egui::Shape::convex_polygon(triangle_points(corner, scale), color, (0.0, color))
}

/// One cell's worth of half-square, for a `Corner` color in the picture: a whole cell's `scale`,
/// positioned at the cell's screen origin.
pub(super) fn half_cell(
    corner: Corner,
    fill: Color32,
    to_screen: &egui::emath::RectTransform,
    origin: Pos2,
) -> egui::Shape {
    let mut half = triangle_shape(corner, fill, to_screen.scale());
    half.translate(origin.to_vec2());
    half
}

/// One clue box's corners as it is drawn: a cap's triangle, or the whole box.
fn clue_box_corners(color_info: &ColorInfo, rect: Rect) -> Vec<Pos2> {
    match color_info.corner {
        Some(corner) => triangle_points(corner, rect.size())
            .iter()
            .map(|p| *p + rect.min.to_vec2())
            .collect(),
        None => vec![
            rect.left_top(),
            rect.right_top(),
            rect.right_bottom(),
            rect.left_bottom(),
        ],
    }
}

/// A capped clue is a shape as much as a number — the caps say which way its ends slant — so it's
/// drawn as one silhouette across all of its boxes: filled while there's still work in it, and
/// outlined once it's resolved.
///
/// This goes down *before* the boxes that sit on it, so that the seams between them don't show.
pub(super) fn draw_clue_silhouette(
    ui: &egui::Ui,
    painter: &egui::Painter,
    boxes: &[(&ColorInfo, Option<u16>, Rect)],
    fixed: bool,
    scale: f32,
    rgb: (u8, u8, u8),
) {
    let corners = boxes
        .iter()
        .flat_map(|(color_info, _, rect)| clue_box_corners(color_info, *rect))
        .collect();
    if fixed {
        draw_clue_outline(ui, painter, corners, scale, rgb);
    } else {
        painter.add(egui::Shape::convex_polygon(
            convex_hull(corners),
            Color32::from_rgb(rgb.0, rgb.1, rgb.2),
            egui::Stroke::NONE,
        ));
    }
}

/// One cap of a clue in the gutter: the half-box triangle, filling `rect`'s share of the
/// silhouette. A resolved clue's caps are left to `draw_clue_silhouette`'s outline instead.
pub(super) fn draw_cap(painter: &egui::Painter, color_info: &ColorInfo, rect: Rect) {
    let (r, g, b) = color_info.rgb;
    let mut triangle = triangle_shape(
        color_info.corner.expect("must be a corner"),
        Color32::from_rgb(r, g, b),
        rect.size(),
    );
    triangle.translate(rect.min.to_vec2());
    painter.add(triangle);
}

/// The outline a resolved triano clue keeps in place of its filled boxes: its whole silhouette,
/// caps included, so it still reads as one clue rather than a loose number between two gaps.
fn draw_clue_outline(
    ui: &egui::Ui,
    painter: &egui::Painter,
    corners: Vec<Pos2>,
    scale: f32,
    rgb: (u8, u8, u8),
) {
    let hull = convex_hull(corners);
    if hull.len() < 3 {
        return; // Nothing with an inside; not a shape we can outline.
    }
    let width = clue_outline_width(scale);
    // A pale outline against a pale panel would vanish, so it gets the same backing a bare
    // number's glyphs do.
    if let Some(halo) = contrast_outline(ui, rgb) {
        painter.add(egui::Shape::closed_line(
            hull.clone(),
            egui::Stroke::new(width * 2.0, halo),
        ));
    }
    painter.add(egui::Shape::closed_line(
        hull,
        egui::Stroke::new(width, Color32::from_rgb(rgb.0, rgb.1, rgb.2)),
    ));
}

/// `layout::convex_hull`, in egui's coordinates.
fn convex_hull(points: Vec<Pos2>) -> Vec<Pos2> {
    let hull = crate::layout::convex_hull(
        points
            .iter()
            .map(|p| crate::layout::Point::new(p.x, p.y))
            .collect(),
    );
    hull.iter().map(|p| Pos2::new(p.x, p.y)).collect()
}
