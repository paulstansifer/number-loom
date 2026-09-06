//! HTML export: a printable puzzle, drawn as inline SVG.
//!
//! Everything here works from the same abstract-unit geometry the editor canvas draws from:
//! `Geometry::rows` for the cells, `guides` for the grid lines, `gutters` for where the clues go,
//! and `Clue::express` for what a clue is made of. So this module is generic over both the clue
//! style and the grid shape, and the shapes it emits are the shapes on screen. One abstract unit
//! is one cell edge, which is also the SVG user unit, so every size below reads as a fraction of
//! a cell.

use std::fmt::Write;

use crate::{
    geometry::GridKind,
    layout::{self, CLUE_BOX, CLUE_BOX_SHORT, Point},
    puzzle::{Clue, ClueStyle, ColorInfo, Palette, Puzzle},
};

/// How many CSS pixels one cell is drawn at. Matches the 40px cells the `<table>` export used, and
/// so about matches what a printer will do with it.
const PX_PER_CELL: f32 = 40.0;

/// The two colors the drawing itself uses; the palette supplies the rest.
const INK: &str = "#000";
const PAPER: &str = "#fff";

/// Cell edges and the lighter grid lines.
const THIN_STROKE: f32 = 0.02;
/// Every fifth grid line, and the outline around a clue.
const THICK_STROKE: f32 = 0.05;

/// A clue's digits, as a fraction of a cell. Under `CLUE_BOX` so a number doesn't touch the sides
/// of the box it sits in; the box is `CLUE_BOX` along the gutter.
const CLUE_FONT: f32 = 0.5;

/// Format a float without a trailing `.0`, so the markup stays readable.
fn n(v: f32) -> String {
    let r = (v * 1000.0).round() / 1000.0;
    if r == r.trunc() {
        format!("{}", r as i64)
    } else {
        format!("{r}")
    }
}

fn rgb((r, g, b): (u8, u8, u8)) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// How light a color looks, from 0.0 (black) to 1.0 (white) — the same weighting the GUI uses to
/// decide whether something needs an outline to stay legible.
fn luminance((r, g, b): (u8, u8, u8)) -> f32 {
    (0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32) / 255.0
}

/// A clue drawn in a very pale color would be all but invisible on white paper, so it gets a dark
/// halo stroked around it (`paint-order` puts the stroke behind the fill). `None` when the color
/// stands out on its own.
fn halo(color: (u8, u8, u8)) -> Option<&'static str> {
    (luminance(color) > 0.75).then_some(INK)
}

fn polygon(out: &mut String, points: &[Point], style: &str) {
    let pts: Vec<String> = points
        .iter()
        .map(|p| format!("{},{}", n(p.x), n(p.y)))
        .collect();
    let _ = writeln!(out, r#"<polygon points="{}" {style}/>"#, pts.join(" "));
}

/// A clue's number, centred in its box. Sized down as it gets longer — `textLength` squeezes the
/// glyphs rather than letting a three-digit clue spill out of its box, which is what the GUI's
/// `clue_font` does by picking a narrower font.
fn number(out: &mut String, at: Point, txt: &str, color: (u8, u8, u8)) {
    let fit = if txt.chars().count() > 1 {
        format!(
            r#" textLength="{}" lengthAdjust="spacingAndGlyphs""#,
            n(CLUE_BOX * 0.85)
        )
    } else {
        String::new()
    };
    let outline = match halo(color) {
        Some(c) => format!(
            r#" stroke="{c}" stroke-width="{}" paint-order="stroke""#,
            n(0.03)
        ),
        None => String::new(),
    };
    let _ = writeln!(
        out,
        r#"<text x="{}" y="{}" fill="{}"{outline}{fit}>{}</text>"#,
        n(at.x),
        n(at.y),
        rgb(color),
        escape(txt),
    );
}

/// The only text that reaches the output is clue numbers and the puzzle's own title and author, so
/// this only has to cover what would break the markup.
fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Where one clue box sits: `out` is how far its centre is from the gutter's anchor, along the
/// lane. `square` boxes are for trianogram caps, whose diagonals only make sense in a box that
/// isn't stretched; everything else gets the lane-shaped box the GUI draws, which is a rectangle
/// on a square grid and a rhombus on a triddler.
fn box_at(g: &layout::GutterLane, out: f32, square: bool) -> [Point; 4] {
    let center = Point::new(
        g.anchor.x + g.outward.x * out,
        g.anchor.y + g.outward.y * out,
    );
    if square {
        let half = CLUE_BOX / 2.0;
        [
            Point::new(center.x - half, center.y - half),
            Point::new(center.x + half, center.y - half),
            Point::new(center.x + half, center.y + half),
            Point::new(center.x - half, center.y + half),
        ]
    } else {
        layout::clue_box(center, g.outward, g.edge_dir, CLUE_BOX, CLUE_BOX_SHORT)
    }
}

/// The axis-aligned bounding box of a clue box, for the cap triangles inscribed in it.
fn bounds(points: &[Point]) -> (Point, layout::Vec2) {
    let (mut lo, mut hi) = (points[0], points[0]);
    for p in points {
        lo = Point::new(lo.x.min(p.x), lo.y.min(p.y));
        hi = Point::new(hi.x.max(p.x), hi.y.max(p.y));
    }
    (lo, layout::Vec2::new(hi.x - lo.x, hi.y - lo.y))
}

/// One lane's clues, grouped: each entry is one clue's boxes, as (color, count) pairs in the order
/// they're drawn, counting outward from the grid. A count of `None` is a trianogram cap.
///
/// The same walk `gui::triddler::expressed_clues` does for the screen, except that this keeps the
/// clues grouped rather than flattening them, since a capped clue is drawn as one silhouette.
fn expressed<'a, C: Clue>(
    line: &[C],
    palette: &'a Palette,
    reversed: bool,
) -> Vec<Vec<(&'a ColorInfo, Option<u16>)>> {
    let mut v: Vec<Vec<(&ColorInfo, Option<u16>)>> =
        line.iter().map(|c| c.express(palette)).collect();
    // Clues run in the lane's own direction, so the clue nearest the grid is the last one;
    // `reversed` covers the families whose clues are labelled at the far end from where the lane
    // is stored. Both levels reverse: the parts of a clue are ordered like the clue itself.
    if !reversed {
        v.reverse();
        for clue in &mut v {
            clue.reverse();
        }
    }
    v
}

/// Draw one lane's clues into the gutter.
///
/// A trianogram clue is a shape as much as a number — its caps say which way its ends slant — so
/// the whole clue is outlined as one silhouette, exactly as the solve view draws a resolved clue.
/// That is also why the boxes within one clue abut and only *clues* are separated by `CLUE_GAP`:
/// `◢2◤` describes four consecutive cells, and a gap between the cap and the body would say
/// otherwise. An ordinary clue is a single box, so the same rule leaves it spaced as usual.
fn draw_lane<C: Clue>(
    out: &mut String,
    line: &[C],
    palette: &Palette,
    g: &layout::GutterLane,
    shaped: bool,
) {
    // The centre of the first box: past the indicator strip, half a box further on.
    let mut at = crate::layout::CLUE_PAD + CLUE_BOX / 2.0;

    for clue in expressed(line, palette, g.reversed) {
        let boxes: Vec<[Point; 4]> = (0..clue.len())
            .map(|i| box_at(g, at + i as f32 * CLUE_BOX, shaped))
            .collect();
        at += clue.len() as f32 * CLUE_BOX + crate::layout::CLUE_GAP;

        // The body's color speaks for the whole clue; a clue that is nothing but caps falls back
        // to the first of those.
        let ink = clue
            .iter()
            .find(|(_, count)| count.is_some())
            .or(clue.first())
            .map(|(ci, _)| ci.rgb)
            .unwrap_or((0, 0, 0));

        if shaped {
            // Every corner of every box the clue covers; the hull drops the seams between them.
            let mut corners = vec![];
            for ((ci, _), points) in clue.iter().zip(&boxes) {
                match ci.corner {
                    Some(corner) => {
                        let (origin, size) = bounds(points);
                        let (tri, tn) =
                            layout::corner_triangle(corner.upper, corner.left, origin, size);
                        corners.extend_from_slice(&tri[..tn]);
                    }
                    None => corners.extend_from_slice(points),
                }
            }
            let hull = layout::convex_hull(corners);
            // Fewer than three corners has no inside; nothing to outline.
            if hull.len() >= 3 {
                polygon(
                    out,
                    &hull,
                    &format!(
                        r#"fill="{PAPER}" stroke="{}" stroke-width="{}" stroke-linejoin="round""#,
                        rgb(ink),
                        n(THICK_STROKE)
                    ),
                );
            }
        }

        for ((ci, count), points) in clue.iter().zip(&boxes) {
            // A cap carries no number; the silhouette above is the whole of it.
            let Some(len) = count else { continue };
            if !shaped {
                // Filled with paper, not left open: a triddler's boxes are rhombuses whose
                // corners reach past their neighbours along the gutter, so open outlines would
                // cross each other instead of reading as a chain.
                polygon(
                    out,
                    points,
                    &format!(
                        r#"fill="{PAPER}" stroke="{}" stroke-width="{}""#,
                        rgb(ink),
                        n(THICK_STROKE)
                    ),
                );
            }
            number(out, centroid(points), &len.to_string(), ci.rgb);
        }
    }
}

fn centroid(points: &[Point]) -> Point {
    let (sx, sy) = points
        .iter()
        .fold((0.0, 0.0), |(x, y), p| (x + p.x, y + p.y));
    Point::new(sx / points.len() as f32, sy / points.len() as f32)
}

/// A printable puzzle: an HTML page wrapping one inline SVG drawing.
pub fn as_html<C: Clue, K: GridKind>(puzzle: &Puzzle<C, K>, title: &str, author: &str) -> String {
    let (lo, hi) = puzzle.drawing_bounds();
    let (w, h) = (hi.x - lo.x, hi.y - lo.y);

    let mut body = String::new();

    // The cells: empty boxes to be filled in. Their own edges are the fine grid lines, so a
    // triddler's triangles come out lined the same way a square grid's cells do.
    let _ = writeln!(
        body,
        r#"<g fill="{PAPER}" stroke="{INK}" stroke-width="{}">"#,
        n(THIN_STROKE)
    );
    for row in puzzle.geometry.rows() {
        for drawn in row.cells() {
            let (verts, vn) = drawn.shape.vertices(drawn.origin);
            polygon(&mut body, &verts[..vn], "");
        }
    }
    let _ = writeln!(body, "</g>");

    // Every fifth boundary, drawn heavier over the top — for a triddler that means every fifth
    // lane *within a family*, which is what makes a hexagon countable by eye.
    let _ = writeln!(
        body,
        r#"<g stroke="{INK}" stroke-width="{}" stroke-linecap="square">"#,
        n(THICK_STROKE)
    );
    for guide in puzzle.geometry.guides().iter().filter(|g| g.emphasis) {
        let _ = writeln!(
            body,
            r#"<line x1="{}" y1="{}" x2="{}" y2="{}"/>"#,
            n(guide.from.x),
            n(guide.from.y),
            n(guide.to.x),
            n(guide.to.y)
        );
    }
    let _ = writeln!(body, "</g>");

    // The clues, in whichever gutters this shape has: two for a square puzzle, six for a triddler.
    let shaped = C::style() == ClueStyle::Triano;
    let _ = writeln!(
        body,
        r#"<g font-size="{}" text-anchor="middle" dominant-baseline="central" font-family="DejaVu Sans Mono, Menlo, Consolas, monospace">"#,
        n(CLUE_FONT)
    );
    for (_, gutter) in puzzle.geometry.gutters() {
        for g in gutter {
            draw_lane(&mut body, &puzzle.lines[g.lane], &puzzle.palette, g, shaped);
        }
    }
    let _ = writeln!(body, "</g>");

    let heading = [title, author]
        .iter()
        .filter(|s| !s.is_empty())
        .map(|s| escape(s))
        .collect::<Vec<_>>()
        .join(" — ");
    let heading = if heading.is_empty() {
        String::new()
    } else {
        format!("<h1>{heading}</h1>\n")
    };

    format!(
        r#"<!DOCTYPE html>
<html>
<head>
<meta charset="utf-8"/>
<title>{title}</title>
<style>
body {{ margin: 1em; font-family: sans-serif; }}
h1 {{ font-size: 1em; font-weight: normal; }}
svg {{ max-width: 100%; height: auto; }}
@media print {{ body {{ margin: 0; }} }}
</style>
</head>
<body>
{heading}<svg xmlns="http://www.w3.org/2000/svg" viewBox="{vx} {vy} {vw} {vh}" width="{pw}" height="{ph}">
{body}</svg>
</body>
</html>
"#,
        title = escape(title),
        vx = n(lo.x),
        vy = n(lo.y),
        vw = n(w),
        vh = n(h),
        pw = n(w * PX_PER_CELL),
        ph = n(h * PX_PER_CELL),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::puzzle::{Document, DynPuzzle};

    /// Everything the SVG draws, as flat coordinate pairs: polygon corners and line endpoints.
    fn drawn_points(svg: &str) -> Vec<(f32, f32)> {
        let mut out = vec![];
        for chunk in svg.split("points=\"").skip(1) {
            for pair in chunk[..chunk.find('"').unwrap()].split_whitespace() {
                let (x, y) = pair.split_once(',').unwrap();
                out.push((x.parse().unwrap(), y.parse().unwrap()));
            }
        }
        for line in svg.split("<line ").skip(1) {
            let attr = |name: &str| -> f32 {
                let at = line.find(&format!("{name}=\"")).unwrap() + name.len() + 2;
                line[at..][..line[at..].find('"').unwrap()].parse().unwrap()
            };
            out.push((attr("x1"), attr("y1")));
            out.push((attr("x2"), attr("y2")));
        }
        out
    }

    fn view_box(svg: &str) -> (f32, f32, f32, f32) {
        let at = svg.find("viewBox=\"").unwrap() + 9;
        let nums: Vec<f32> = svg[at..][..svg[at..].find('"').unwrap()]
            .split_whitespace()
            .map(|n| n.parse().unwrap())
            .collect();
        (nums[0], nums[1], nums[2], nums[3])
    }

    fn load(file: &str) -> Document {
        crate::import::load_path(&std::path::PathBuf::from(file), None).unwrap()
    }

    fn export(file: &str) -> String {
        let mut doc = load(file);
        let (title, author) = (doc.title.clone(), doc.author.clone());
        crate::with_puzzle!(doc.puzzle(), |p| as_html(p, &title, &author))
    }

    /// The whole point of `drawing_bounds`: nothing the exporter draws may fall outside the
    /// viewBox, or a clue would simply be missing from the printed page. A triddler is the case
    /// that matters — its six gutters run off past every side of the picture.
    #[test]
    fn nothing_is_drawn_outside_the_view_box() {
        for file in [
            "examples/triddler/blob.g",
            "examples/char-grid/ladle.txt",
            "examples/char-grid/usb_type_a_no_emblem.txt",
            "examples/char-grid/unicode_colors.txt",
        ] {
            let svg = export(file);
            let (vx, vy, vw, vh) = view_box(&svg);
            let points = drawn_points(&svg);
            assert!(!points.is_empty(), "{file} drew nothing at all");
            for (x, y) in points {
                assert!(
                    x >= vx - 1e-3 && x <= vx + vw + 1e-3 && y >= vy - 1e-3 && y <= vy + vh + 1e-3,
                    "{file}: ({x}, {y}) falls outside the viewBox {vx} {vy} {vw} {vh}"
                );
            }
        }
    }

    /// Every clue reaches the page. Counting `<text>` elements catches a gutter that was skipped
    /// or a lane whose clues were walked in a way that dropped some.
    #[test]
    fn every_clue_gets_a_number() {
        for file in ["examples/triddler/blob.g", "examples/char-grid/ladle.txt"] {
            let mut doc = load(file);
            // A cap has no number of its own, so only the parts with counts should be drawn.
            let expected: usize = crate::with_puzzle!(doc.puzzle(), |p| {
                p.lines
                    .iter()
                    .flatten()
                    .flat_map(|c| c.express(&p.palette))
                    .filter(|(_, count)| count.is_some())
                    .count()
            });
            let svg = export(file);
            assert_eq!(
                svg.matches("</text>").count(),
                expected,
                "{file} drew the wrong number of clues"
            );
        }
    }

    /// A trianogram's caps come out as shapes, not as the `◢`/`◤` characters the old `<table>`
    /// export printed next to the number.
    #[test]
    fn trianogram_caps_are_shapes() {
        let svg = export("examples/char-grid/ladle.txt");
        assert!(!svg.contains('◢') && !svg.contains('◤') && !svg.contains('◣'));
        // Three-cornered clue silhouettes: a capped clue's hull has a slanted side, so at least
        // some polygons have an odd number of corners for a shape made of rectangles.
        let odd = drawn_points(&svg);
        assert!(!odd.is_empty());
        assert!(
            svg.matches("<polygon").count() > svg.matches("</text>").count(),
            "capped clues should draw more outlines than numbers"
        );
    }

    /// A triddler reaches the HTML writer at all — it used to be refused outright.
    #[test]
    fn triddlers_export() {
        let mut doc = load("examples/triddler/blob.g");
        assert!(matches!(doc.puzzle(), DynPuzzle::TriNono(_)));
        let bytes = crate::export::to_bytes(
            &mut doc,
            Some("x.html".to_string()),
            Some(crate::puzzle::NonogramFormat::Html),
        )
        .unwrap();
        let out = String::from_utf8(bytes).unwrap();
        assert!(out.contains("<svg"));
        // Six gutters' worth of clues, not two.
        assert!(out.matches("</text>").count() > 20);
    }
}
