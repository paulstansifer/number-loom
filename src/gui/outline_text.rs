//! The halo behind a number written straight onto the picture.
//!
//! Sometimes text needs to be visible on arbitrary backgrounds, and `egui` doesn't support text
//! outlining.
//!
//! So we take the digit's shape out of the font and stroke it. The stroke is centred on the contour,
//! so half of it hides under the glyph when the text is painted on top: it cannot detach, it is the
//! same thickness the whole way round, and it stays right at any zoom.
//!
//! Everything about the font is worked out once and cached in unscaled font units; drawing a
//! number is then an affine transform of a couple of hundred points.

use std::sync::LazyLock;

use ab_glyph::Font as _;
use egui::{Align2, Color32, Pos2, Shape, Stroke, Vec2, emath::GuiRounding as _};

/// How many line segments each Bézier in an outline becomes. Honestly, 1 only has slight artifacts.
const SEGMENTS_PER_CURVE: usize = 3;

/// Two outline points this close together (in font units, where an em is ~2048) are the same
/// point — which is how a contour is recognised as having closed.
const SAME_POINT: f32 = 0.01;

/// The very bytes egui rasterizes `FontFamily::Monospace` from, so the outlines we draw are the
/// outlines it draws. (If `epaint_default_fonts` ever falls out of step with the `egui` version,
/// the halos would quietly stop fitting — hence the test at the bottom of this file.)
static HACK: LazyLock<ab_glyph::FontRef<'static>> = LazyLock::new(|| {
    ab_glyph::FontRef::try_from_slice(epaint_default_fonts::HACK_REGULAR)
        .expect("epaint's own bundled monospace font should parse")
});

/// A digit's outline, every contour already flattened.
struct DigitOutline {
    /// Closed polylines in unscaled font units — the digit's silhouette, and the counters
    /// inside `0`, `4`, `6`, `8` and `9` too, which is what keeps a black number legible
    /// when it lands on a black cell.
    contours: Vec<Vec<ab_glyph::Point>>,
}

/// `'0'` through `'9'`, worked out on first use and then never again. Nothing else is ever drawn
/// this way — both callers are printing a count.
static DIGITS: LazyLock<[Option<DigitOutline>; 10]> =
    LazyLock::new(|| std::array::from_fn(|d| digit_outline(char::from(b'0' + d as u8))));

fn digit_outline(chr: char) -> Option<DigitOutline> {
    let outline = HACK.outline(HACK.glyph_id(chr))?;
    Some(DigitOutline {
        contours: contours(&outline),
    })
}

fn same_point(a: ab_glyph::Point, b: ab_glyph::Point) -> bool {
    (a.x - b.x).abs() < SAME_POINT && (a.y - b.y).abs() < SAME_POINT
}

fn curve_start(curve: &ab_glyph::OutlineCurve) -> ab_glyph::Point {
    use ab_glyph::OutlineCurve::*;
    match curve {
        Line(p0, _) | Quad(p0, _, _) | Cubic(p0, _, _, _) => *p0,
    }
}

fn curve_end(curve: &ab_glyph::OutlineCurve) -> ab_glyph::Point {
    use ab_glyph::OutlineCurve::*;
    match curve {
        Line(_, p) | Quad(_, _, p) | Cubic(_, _, _, p) => *p,
    }
}

/// A weighted sum of points: a Bézier evaluated against its Bernstein coefficients.
/// (`ab_glyph::Point` has no scalar multiplication of its own.)
fn weigh(terms: &[(ab_glyph::Point, f32)]) -> ab_glyph::Point {
    terms.iter().fold(ab_glyph::point(0.0, 0.0), |acc, (p, w)| {
        ab_glyph::point(acc.x + p.x * w, acc.y + p.y * w)
    })
}

/// A curve's start point and its interior, but *not* its end — the next curve in the contour
/// starts there, and the last one ends back where the contour began.
fn push_curve(curve: &ab_glyph::OutlineCurve, out: &mut Vec<ab_glyph::Point>) {
    use ab_glyph::OutlineCurve::*;
    out.push(curve_start(curve));
    let steps = match curve {
        Line(..) => 0,
        Quad(..) | Cubic(..) => SEGMENTS_PER_CURVE,
    };
    for i in 1..steps {
        let t = i as f32 / steps as f32;
        let (u, tt, uu) = (1.0 - t, t * t, (1.0 - t) * (1.0 - t));
        out.push(match curve {
            Line(..) => unreachable!("a straight line needs no interior points"),
            Quad(p0, p1, p2) => weigh(&[(*p0, uu), (*p1, 2.0 * u * t), (*p2, tt)]),
            Cubic(p0, p1, p2, p3) => weigh(&[
                (*p0, uu * u),
                (*p1, 3.0 * uu * t),
                (*p2, 3.0 * u * tt),
                (*p3, tt * t),
            ]),
        });
    }
}

/// Split a glyph's flat list of curves into its separate closed contours.
///
/// `ab_glyph` hands them over with no delimiter between them, but its builder emits an explicit
/// closing line back to each contour's starting point — so a contour is over exactly when the
/// chain arrives back where it set out from.
fn contours(outline: &ab_glyph::Outline) -> Vec<Vec<ab_glyph::Point>> {
    let mut done = Vec::new();
    let mut current: Vec<ab_glyph::Point> = Vec::new();
    let mut start = None;

    for curve in &outline.curves {
        let start = *start.get_or_insert(curve_start(curve));
        push_curve(curve, &mut current);
        if same_point(curve_end(curve), start) {
            done.push(std::mem::take(&mut current));
        }
    }
    // A glyph that never closed its last contour; treat it as implicitly closed.
    if !current.is_empty() {
        done.push(current);
    }
    done.retain(|contour| contour.len() >= 3);
    done
}

/// Cut the sharp corners off a contour, so that stroking it doesn't grow spikes.
///
/// epaint miters every join on a closed path and applies no miter limit — its corner cut-off is
/// switched off (`CUT_OFF_SHARP_CORNERS` in its tessellator, disabled over a bug in *filled*
/// shapes) — so a join extends `(width / 2) / sin(angle / 2)` past the corner, without bound as
/// the corner sharpens. Hack's `4` has an acute apex, and `1` a shallower one on its flag.
///
/// Replacing such a corner with two shallower ones fixes it whatever `cut` is, since a cut corner
/// leaves joins of at least a right angle, whose miters can't reach past the stroke's own width.
/// `cut` stays well under `width / 2` so that the tip of the glyph itself is still inside its halo.
fn bevel_sharp_corners(points: &[Pos2], cut: f32) -> Vec<Pos2> {
    let n = points.len();
    let mut out = Vec::with_capacity(n + 8);
    for i in 0..n {
        let corner = points[i];
        let (before, after) = (
            points[(i + n - 1) % n] - corner,
            points[(i + 1) % n] - corner,
        );
        let (before_len, after_len) = (before.length(), after.length());
        if before_len <= f32::EPSILON || after_len <= f32::EPSILON {
            out.push(corner);
            continue;
        }
        let (before, after) = (before / before_len, after / after_len);
        // A positive dot product means the two edges leave at less than a right angle.
        if before.dot(after) <= 0.0 {
            out.push(corner);
            continue;
        }
        // Never eat more than half of either edge, or two neighbouring cuts would cross.
        let cut = cut.min(before_len / 2.0).min(after_len / 2.0);
        out.push(corner + before * cut);
        out.push(corner + after * cut);
    }
    out
}

/// The sub-pixel offset epaint renders a glyph at, given where its layout put it in physical
/// pixels: `SubpixelBin::new`, which rounds to the nearest quarter pixel.
fn subpixel_bin(h_pos: f32) -> f32 {
    let quarters = (h_pos.rem_euclid(1.0) * 4.0).round();
    (quarters % 4.0) / 4.0
}

/// The halo for a number, as shapes to add to the painter just before the text itself.
///
/// `center`, `txt` and `font` must be the ones the following `painter.text` is given, with
/// `Align2::CENTER_CENTER`; the halo is laid out from the same galley, so it lands exactly on the
/// glyphs egui goes on to draw. `width` is the whole stroke, half of which ends up hidden under
/// the text — so a number stands `width / 2.0` proud of its border.
pub(crate) fn halo_shapes(
    ui: &egui::Ui,
    center: Pos2,
    txt: &str,
    font: &egui::FontId,
    color: Color32,
    width: f32,
) -> Vec<Shape> {
    let galley = ui.fonts_mut(|f| f.layout_no_wrap(txt.to_owned(), font.clone(), color));
    let ppp = ui.ctx().pixels_per_point();
    // `Painter::text` places a galley exactly this way, and then the tessellator snaps it to the
    // pixel grid (`round_text_to_pixels`).
    let origin = Align2::CENTER_CENTER
        .anchor_size(center, galley.size())
        .min
        .round_to_pixels(ppp);
    // The size epaint rasterizes at, from `FontFace::styled_metrics`: pixels per em, unrounded.
    // Hack's `FontTweak` is the default one, so there's no tweak factor to fold in here.
    let Some(units_per_em) = HACK.units_per_em() else {
        return Vec::new();
    };
    let px_per_unit = font.size * ppp / units_per_em;
    if px_per_unit <= 0.0 {
        return Vec::new();
    }
    let stroke = Stroke::new(width, color);

    let mut shapes = Vec::new();
    for row in &galley.rows {
        // Where epaint's layout cursor was, in physical pixels, which is what picks the
        // sub-pixel bin the glyph gets rendered at (`GlyphAtlas::allocate_outline`).
        let mut cursor_px = 0.0;
        for glyph in &row.glyphs {
            let h_pos = cursor_px;
            cursor_px += glyph.advance_width * ppp;
            let Some(digit) = glyph
                .chr
                .to_digit(10)
                .and_then(|d| DIGITS[d as usize].as_ref())
            else {
                continue; // Not a digit, or a glyph the font has no outline for: no halo.
            };
            let bin = subpixel_bin(h_pos);

            // The glyph's pen position — the point on the baseline that the outline is drawn
            // from — on screen. `uv_rect.offset` is the rasterized bitmap's top-left corner
            // relative to that, and it's a whole number of pixels, so it can come off again
            // after `tessellate_glyphs` has snapped the corner to a pixel. Going by the bitmap's
            // corner itself would be at the mercy of the hinter, which moves a digit's top by up
            // to a pixel; the baseline stays put.
            let pen = origin
                + row.pos.to_vec2()
                + (glyph.pos.to_vec2() + glyph.uv_rect.offset).round_to_pixels(ppp)
                - glyph.uv_rect.offset;
            // `FontCell::rasterize_glyph` draws the outline shifted right by the bin. Font units
            // are y-up and scaled to pixels; the canvas is y-down and in points.
            let to_screen = |p: &ab_glyph::Point| {
                pen + Vec2::new(p.x * px_per_unit + bin, -p.y * px_per_unit) / ppp
            };

            for contour in &digit.contours {
                let points: Vec<Pos2> = contour.iter().map(to_screen).collect();
                shapes.push(Shape::closed_line(
                    bevel_sharp_corners(&points, width / 4.0),
                    stroke,
                ));
            }
        }
    }
    shapes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_test_ui(
        pixels_per_point: f32,
        font_hinting: bool,
        add_contents: impl Fn(&mut egui::Ui),
    ) {
        // Not `egui::__run_test_ui`: that one installs *no* fonts, and fonts are the point here.
        let ctx = egui::Context::default();
        ctx.set_pixels_per_point(pixels_per_point);
        ctx.global_style_mut(|style| style.visuals.text_options.font_hinting = font_hinting);
        ctx.run_ui(Default::default(), |ui| {
            egui::CentralPanel::default().show(ui, |ui| add_contents(ui));
        })
        .drop_without_applying_deltas();
    }

    /// Where `txt`'s ink really ends up when `painter.text` draws it centered on `center`: the
    /// bounds of the mesh epaint tessellates it into, after all of epaint's snapping to pixels.
    fn drawn_ink(ui: &egui::Ui, center: Pos2, txt: &str, font: &egui::FontId) -> egui::Rect {
        let galley =
            ui.fonts_mut(|f| f.layout_no_wrap(txt.to_owned(), font.clone(), Color32::WHITE));
        let origin = Align2::CENTER_CENTER.anchor_size(center, galley.size()).min;
        let shape = egui::epaint::ClippedShape {
            clip_rect: egui::Rect::EVERYTHING,
            shape: Shape::galley(origin, galley, Color32::WHITE),
            transform_after_tessellation: egui::emath::TSTransform::IDENTITY,
        };
        let mut ink = egui::Rect::NOTHING;
        for primitive in ui
            .ctx()
            .tessellate(vec![shape], ui.ctx().pixels_per_point())
        {
            if let egui::epaint::Primitive::Mesh(mesh) = primitive.primitive {
                for vertex in &mesh.vertices {
                    ink.extend_with(vertex.pos);
                }
            }
        }
        ink
    }

    /// Check every number's halo against the ink egui draws, at all sorts of sizes, scales and
    /// positions. Each edge of the halo should be inside the ink's by up to a pixel and a half
    /// (the bitmap egui rasterizes into is the ink box rounded outwards to whole pixels, and
    /// flattening a curve or cutting a sharp corner off pulls in a little more), and the top and
    /// bottom can also poke out by `vertical_leeway` pixels.
    ///
    /// This is the guard against `epaint_default_fonts` drifting out of step with `egui`, or
    /// against epaint changing how it picks a rasterization size or snaps text to pixels: any
    /// of them would leave the halos subtly misplaced, with nothing else to complain about it.
    fn check_halos(font_hinting: bool, vertical_leeway: f32) {
        let numbers: Vec<String> = (0..10)
            .chain([10, 25, 47, 99, 108])
            .map(|n| n.to_string())
            .collect();
        // Clues are centered wherever their cells fall, which is rarely on a pixel boundary.
        let centers = [
            (100.0, 100.0),
            (100.3, 100.6),
            (100.5, 100.5),
            (100.7, 100.2),
        ];
        for pixels_per_point in [1.0, 1.25, 2.0] {
            with_test_ui(pixels_per_point, font_hinting, |ui| {
                for size in [10.0, 24.0, 60.0] {
                    let font = egui::FontId::monospace(size);
                    for txt in &numbers {
                        for (x, y) in centers {
                            let center = Pos2::new(x, y);
                            let mut halo = egui::Rect::NOTHING;
                            for shape in halo_shapes(ui, center, txt, &font, Color32::WHITE, 1.0) {
                                let egui::Shape::Path(path) = shape else {
                                    panic!("a halo should be nothing but paths");
                                };
                                for point in &path.points {
                                    halo.extend_with(*point);
                                }
                            }
                            assert!(halo.is_finite(), "no halo at all for {txt:?} at {size}");
                            let ink = drawn_ink(ui, center, txt, &font);

                            for (what, off, leeway) in [
                                ("left", halo.min.x - ink.min.x, 0.0),
                                ("top", halo.min.y - ink.min.y, vertical_leeway),
                                ("right", ink.max.x - halo.max.x, 0.0),
                                ("bottom", ink.max.y - halo.max.y, vertical_leeway),
                            ] {
                                let off = off * pixels_per_point;
                                // Plus a little for floating-point noise.
                                assert!(
                                    (-leeway - 0.01..=1.5).contains(&off),
                                    "{txt:?} at {size}pt, centered on {center:?}, \
                                     {pixels_per_point} px/pt, hinting {font_hinting}: halo's \
                                     {what} edge is {off} px off the glyph's \
                                     (want -{leeway}..=1.5)",
                                );
                            }
                        }
                    }
                }
            });
        }
    }

    /// Unhinted, the halo is the very outline egui rasterizes, so it can't be off at all.
    #[test]
    fn halo_lands_on_the_unhinted_glyph() {
        check_halos(false, 0.0);
    }

    /// egui hints its glyphs, and the halo is the unhinted outline: the hinter snaps a digit's
    /// top (and a round one's overshoot below the baseline) to the pixel grid, which can leave the
    /// halo poking out above or below the ink by up to about a pixel. It never moves anything
    /// sideways.
    #[test]
    fn halo_lands_on_the_hinted_glyph() {
        check_halos(true, 1.1);
    }
}
