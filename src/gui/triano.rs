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

/// A corner triangle's corners, in a box of `scale` at the origin: two full sides meeting at the
/// right angle, and a hypotenuse across the other two.
fn triangle_points(corner: Corner, scale: Vec2) -> Vec<Pos2> {
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

    points
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

/// The corners of the smallest convex polygon containing `points` (Andrew's monotone chain),
/// counter-clockwise. Fewer than three distinct points have no outline to draw, and come back as
/// they are.
fn convex_hull(mut points: Vec<Pos2>) -> Vec<Pos2> {
    if points.len() < 3 {
        return points;
    }
    points.sort_by(|a, b| {
        (a.x, a.y)
            .partial_cmp(&(b.x, b.y))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let cross = |o: Pos2, a: Pos2, b: Pos2| (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x);

    let mut hull: Vec<Pos2> = Vec::with_capacity(points.len() + 1);
    // The lower chain, then the upper one; each drops any corner that doesn't actually turn, and
    // stops short of eating into the chain before it.
    for pass in 0..2 {
        let base = hull.len();
        for &p in points.iter() {
            while hull.len() >= base + 2
                && cross(hull[hull.len() - 2], hull[hull.len() - 1], p) <= 0.0
            {
                hull.pop();
            }
            hull.push(p);
        }
        hull.pop(); // Where this chain ends is where the next one starts.
        if pass == 0 {
            points.reverse();
        }
    }
    hull
}

#[cfg(test)]
mod hull_tests {
    use super::*;

    fn hull(points: &[(f32, f32)]) -> Vec<(f32, f32)> {
        let mut out: Vec<(f32, f32)> =
            convex_hull(points.iter().map(|(x, y)| Pos2::new(*x, *y)).collect())
                .iter()
                .map(|p| (p.x, p.y))
                .collect();
        // The hull is a cycle, so normalize where it starts to compare it.
        if let Some(first) = out
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap())
            .map(|(i, _)| i)
        {
            out.rotate_left(first);
        }
        out
    }

    /// A resolved triano clue hands over every corner of every box it covers; what comes back is
    /// the silhouette, with the seams between the boxes gone.
    #[test]
    fn hull_of_a_capped_clue() {
        // A cap slanting in, a square body, a cap slanting out — three unit boxes in a row.
        let cap_in = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0)];
        let body = [(1.0, 0.0), (2.0, 0.0), (2.0, 1.0), (1.0, 1.0)];
        let cap_out = [(2.0, 0.0), (3.0, 0.0), (2.0, 1.0)];
        let points: Vec<(f32, f32)> = cap_in
            .iter()
            .chain(&body)
            .chain(&cap_out)
            .copied()
            .collect();

        assert_eq!(
            hull(&points),
            vec![(0.0, 0.0), (3.0, 0.0), (2.0, 1.0), (1.0, 1.0)],
            "the seams between the three boxes shouldn't survive"
        );
    }

    /// A capless clue is a plain box, and keeps its four corners.
    #[test]
    fn hull_of_a_square_clue() {
        assert_eq!(
            hull(&[(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]),
            vec![(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]
        );
    }

    /// Nothing here has an inside to outline, but nothing panics either.
    #[test]
    fn degenerate_hulls() {
        assert!(hull(&[]).is_empty());
        assert_eq!(hull(&[(1.0, 1.0)]), vec![(1.0, 1.0)]);
        assert_eq!(hull(&[(1.0, 1.0), (2.0, 2.0)]).len(), 2);
        // Collinear, and repeated points: fewer than three corners come back, so
        // `draw_clue_outline` draws nothing.
        assert!(hull(&[(0.0, 0.0), (1.0, 1.0), (2.0, 2.0)]).len() < 3);
        assert!(hull(&[(1.0, 1.0), (1.0, 1.0), (1.0, 1.0)]).len() < 3);
    }
}
