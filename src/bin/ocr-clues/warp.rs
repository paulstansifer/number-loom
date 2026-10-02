//! Straightening out a photo of a puzzle on paper that isn't flat, or isn't square-on to the
//! camera, so the rest of the work can assume straight, evenly spaced lines.
//!
//! Each of the grid's lines is followed as a curve: from a starting point, step along it half a
//! cell at a time, looking a little to either side of where it's heading for where the line
//! actually is. (Filled cells can hide a line for a few cells; it's assumed to carry on in the
//! same direction.) Where the horizontal and vertical curves cross are the corners of the cells,
//! and each cell is then mapped back to a square.

use image::{Rgb, RgbImage};

use crate::grid::{Response, rough_pitch};

/// A traced line: points along it, in order.
pub type Curve = Vec<(f32, f32)>;

pub struct Traced {
    /// The horizontal lines, top to bottom, as `(x, y)` points from left to right, each with its
    /// number (counting across, with gaps where a line was missed).
    pub horizontal: Vec<(i64, Curve)>,
    /// The vertical lines, left to right, as `(x, y)` points from top to bottom.
    pub vertical: Vec<(i64, Curve)>,
    pub pitch: f32,
}

/// The lines along one axis near one place: `across = center + offset + k * pitch + slope *
/// (along - along_center)`, for whole `k`.
#[derive(Clone, Copy, Debug)]
struct Patch {
    slope: f32,
    offset: f32,
    pitch: f32,
    /// How clearly there are evenly spaced lines here: the best comb against the average one.
    strength: f32,
}

/// The lattice of lines along `axis` in the patch around `(along, across)`, `radius` each way.
fn patch(
    response: &Response,
    axis: usize,
    along: f32,
    across: f32,
    radius: f32,
    pitch: f32,
) -> Patch {
    let r = radius as i64;
    // Profile of line response across the patch, for lines heading with `slope`.
    let profile = |slope: f32| -> Vec<f32> {
        let span = (2.0 * radius * (1.0 + slope.abs())) as usize + 1;
        let mut p = vec![0.0; span];
        for da in (-r..=r).step_by(2) {
            for dc in -r..=r {
                let v = response.at(axis, along + da as f32, across + dc as f32);
                if v > 0.0 {
                    let d = dc as f32 - slope * da as f32;
                    let bin = (d + span as f32 / 2.0) as usize;
                    if bin < span {
                        p[bin] += v;
                    }
                }
            }
        }
        p
    };
    let sharpness = |p: &[f32]| p.iter().map(|v| v * v).sum::<f32>();
    let (slope, profile) = (-16..=16)
        .map(|i| (i as f32 * 0.0087).tan()) // half-degree steps, up to 8 degrees
        .map(|slope| (slope, profile(slope)))
        .max_by(|a, b| sharpness(&a.1).total_cmp(&sharpness(&b.1)))
        .unwrap();
    let half = profile.len() as f32 / 2.0;
    // The spacing and position whose comb catches the most.
    let mut best = (0.0, 0.0, pitch);
    let mut total = 0.0;
    let mut count = 0.0;
    for p in ((0.75 * pitch) as usize)..=((1.3 * pitch) as usize) {
        for offset in 0..p {
            let comb: f32 = (offset..profile.len())
                .step_by(p)
                .map(|i| profile[i])
                .sum::<f32>()
                / (profile.len() / p).max(1) as f32;
            total += comb;
            count += 1.0;
            if comb > best.0 {
                best = (comb, offset as f32 - half, p as f32);
            }
        }
    }
    Patch {
        slope,
        offset: best.1,
        pitch: best.2,
        strength: best.0 / (total / count).max(f32::MIN_POSITIVE),
    }
}

/// Patches along `axis`, worked out as needed.
struct Patches<'a> {
    response: &'a Response,
    axis: usize,
    pitch: f32,
    known: std::collections::HashMap<(i64, i64), Patch>,
}

impl Patches<'_> {
    /// Whether there's a solid line from `(a0, c0)` to `(a1, c1)`: one with ink most of the way
    /// along, as a grid line has. (A column of digits lines up like a line too, but with gaps.)
    fn solid(&self, a0: f32, a1: f32, c0: f32, c1: f32) -> bool {
        let n = (a1 - a0).abs().ceil().max(1.0) as usize;
        let strengths: Vec<f32> = (0..=n)
            .map(|i| {
                let t = i as f32 / n as f32;
                let (a, c) = (a0 + (a1 - a0) * t, c0 + (c1 - c0) * t);
                (-1..=1)
                    .map(|d| self.response.at(self.axis, a, c + d as f32))
                    .fold(0.0, f32::max)
            })
            .collect();
        let mut sorted = strengths.clone();
        sorted.sort_by(f32::total_cmp);
        let strong = sorted[sorted.len() * 9 / 10];
        if strong <= 0.0 {
            return false;
        }
        let covered = strengths.iter().filter(|&&s| s > 0.25 * strong).count();
        covered as f32 >= 0.7 * strengths.len() as f32
    }

    /// The patch nearest `(along, across)`, and its center.
    fn near(&mut self, along: f32, across: f32) -> (Patch, f32, f32) {
        let key = (
            (along / self.pitch).round() as i64,
            (across / self.pitch).round() as i64,
        );
        let (a, c) = (key.0 as f32 * self.pitch, key.1 as f32 * self.pitch);
        let (response, axis, pitch) = (self.response, self.axis, self.pitch);
        let patch = *self
            .known
            .entry(key)
            .or_insert_with(|| patch(response, axis, a, c, 2.0 * pitch, pitch));
        (patch, a, c)
    }
}

/// A patch counts if its lines are clear enough.
const CLEAR: f32 = 1.6;

/// Follow the line along `axis` through `(along, across)`, both ways, a cell at a time, keeping
/// to the lines each patch says are there.
fn follow(patches: &mut Patches, along: f32, across: f32, len_along: f32) -> Vec<(f32, f32)> {
    let step = patches.pitch;
    let mut points = vec![(along, across)];
    for direction in [1.0f32, -1.0] {
        let (mut a, mut c, mut slope) = (along, across, patches.near(along, across).0.slope);
        let mut misses = 0;
        let mut side = vec![];
        loop {
            a += direction * step;
            if a < 0.0 || a >= len_along {
                break;
            }
            let predicted = c + direction * slope * step;
            let (patch, pa, pc) = patches.near(a, predicted);
            if patch.strength >= CLEAR {
                // The patch's nearest line, where it is at `a`.
                let base = pc + patch.offset + patch.slope * (a - pa);
                let k = ((predicted - base) / patch.pitch).round();
                let snapped = base + k * patch.pitch;
                if (snapped - predicted).abs() < 0.4 * patch.pitch
                    && patches.solid(
                        a - direction * step,
                        a,
                        snapped - direction * patch.slope * step,
                        snapped,
                    )
                {
                    c = snapped;
                    slope = 0.5 * slope + 0.5 * patch.slope;
                    misses = 0;
                    side.push((a, c));
                    continue;
                }
            }
            // Hidden (under filled cells, say), or the grid has ended: carry on in the same
            // direction for a while, in case it's the former. (Those steps aren't kept: past the
            // grid, its lines are extended by the smooth fit, not by guesswork here.)
            c = predicted;
            misses += 1;
            if misses > 10 {
                break;
            }
        }
        if direction < 0.0 {
            side.reverse();
            side.extend(points);
            points = side;
        } else {
            points.extend(side);
        }
    }
    points
}

/// The `across` position of `curve` at `along` (interpolating, or extending its ends straight).
fn across_at(curve: &Curve, axis: usize, along: f32) -> f32 {
    let key = |p: &(f32, f32)| if axis == 0 { (p.0, p.1) } else { (p.1, p.0) };
    let points: Vec<(f32, f32)> = curve.iter().map(key).collect();
    if points.len() < 2 {
        return points[0].1;
    }
    let i = points
        .partition_point(|p| p.0 < along)
        .clamp(1, points.len() - 1);
    let (a, b) = (points[i - 1], points[i]);
    let t = (along - a.0) / (b.0 - a.0).max(f32::MIN_POSITIVE);
    a.1 + t * (b.1 - a.1)
}

fn length(curve: &Curve) -> f32 {
    match (curve.first(), curve.last()) {
        (Some(a), Some(b)) => ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt(),
        _ => 0.0,
    }
}

/// The lines along `axis`, followed from where they cross the clearest stretch of patches, and
/// numbered.
fn trace_axis(response: &Response, axis: usize, pitch: f32) -> Vec<(i64, Curve)> {
    let (len_along, len_across) = if axis == 0 {
        (response.width as f32, response.height as f32)
    } else {
        (response.height as f32, response.width as f32)
    };
    let mut patches = Patches {
        response,
        axis,
        pitch,
        known: Default::default(),
    };
    // Look for the cross-section with the most clear patches, at a few places.
    let candidates: Vec<f32> = (1..8).map(|i| i as f32 * len_along / 8.0).collect();
    let clear_along = |patches: &mut Patches, a: f32| -> Vec<(f32, Patch, f32, f32)> {
        (0..(len_across / pitch) as usize)
            .map(|i| i as f32 * pitch)
            .map(|c| {
                let (p, pa, pc) = patches.near(a, c);
                (c, p, pa, pc)
            })
            .filter(|(_, p, _, _)| p.strength >= CLEAR)
            .collect()
    };
    let busy = candidates
        .iter()
        .copied()
        .max_by_key(|&a| clear_along(&mut patches, a).len())
        .unwrap_or(len_along / 2.0);
    // Each clear patch there says where the line nearest its middle is.
    let mut seeds: Vec<f32> = clear_along(&mut patches, busy)
        .into_iter()
        .map(|(_, p, pa, pc)| {
            let base = pc + p.offset + p.slope * (busy - pa);
            base + ((pc - base) / p.pitch).round() * p.pitch
        })
        .collect();
    seeds.sort_by(f32::total_cmp);
    seeds.dedup_by(|b, a| (*a - *b).abs() < 0.4 * pitch);
    // Lines between seeds a patch apart would be missed, so add one between any two seeds that
    // are more than a cell and a half apart (the follow will snap it to the real line).
    let mut all = vec![];
    for w in seeds.windows(2) {
        all.push(w[0]);
        let n = ((w[1] - w[0]) / pitch).round() as usize;
        if n > 1 && n < 4 {
            for k in 1..n {
                all.push(w[0] + (w[1] - w[0]) * k as f32 / n as f32);
            }
        }
    }
    all.extend(seeds.last());
    let mut curves: Vec<Curve> = all
        .into_iter()
        .map(|c| follow(&mut patches, busy, c, len_along))
        .map(|points| {
            points
                .into_iter()
                .map(|(a, c)| if axis == 0 { (a, c) } else { (c, a) })
                .collect::<Curve>()
        })
        .filter(|curve| length(curve) > 3.0 * pitch)
        .collect();
    // In order across, and without duplicates (two seeds can end up on one line).
    curves.sort_by(|a, b| across_at(a, axis, busy).total_cmp(&across_at(b, axis, busy)));
    curves
        .dedup_by(|b, a| (across_at(a, axis, busy) - across_at(b, axis, busy)).abs() < 0.4 * pitch);
    // Number them by counting cells from each to the next (so a line that was missed leaves a
    // gap in the numbers, rather than squashing two cells into one).
    let positions: Vec<f32> = curves.iter().map(|c| across_at(c, axis, busy)).collect();
    let mut gaps: Vec<f32> = positions
        .windows(2)
        .map(|w| w[1] - w[0])
        .filter(|&g| g < 1.5 * pitch)
        .collect();
    gaps.sort_by(f32::total_cmp);
    let local = gaps.get(gaps.len() / 2).copied().unwrap_or(pitch);
    let mut k = 0;
    let mut numbered = vec![];
    for (i, curve) in curves.into_iter().enumerate() {
        if i > 0 {
            k += (((positions[i] - positions[i - 1]) / local).round() as i64).max(1);
        }
        numbered.push((k, curve));
    }
    numbered
}

pub fn trace(image: &RgbImage) -> Option<Traced> {
    let response = Response::new(image);
    let pitch = rough_pitch(&response)?;
    let horizontal = trace_axis(&response, 0, pitch);
    let vertical = trace_axis(&response, 1, pitch);
    (horizontal.len() >= 3 && vertical.len() >= 3).then_some(Traced {
        horizontal,
        vertical,
        pitch,
    })
}

/// A smooth family of lines: `across = f(along, k)` for the line numbered `k`, as a polynomial
/// of degree `DEGREE` in each (after scaling both to about -1..1).
struct Family {
    coefficients: Vec<f32>,
    along_scale: f32,
    k_center: f32,
    k_scale: f32,
}

const DEGREE: usize = 2;

impl Family {
    fn terms(&self, along: f32, k: f32) -> Vec<f32> {
        let a = along / self.along_scale * 2.0 - 1.0;
        let k = (k - self.k_center) / self.k_scale;
        let mut terms = vec![];
        for i in 0..=DEGREE {
            for j in 0..=DEGREE {
                terms.push(a.powi(i as i32) * k.powi(j as i32));
            }
        }
        terms
    }

    fn at(&self, along: f32, k: f32) -> f32 {
        self.terms(along, k)
            .iter()
            .zip(&self.coefficients)
            .map(|(t, c)| t * c)
            .sum()
    }

    /// Fit to `(along, k, across)` points, leaving out the ones that fit worst (where tracing
    /// went astray) and fitting again.
    fn fit(points: &[(f32, f32, f32)], along_scale: f32, pitch: f32) -> Option<Family> {
        let (k_min, k_max) = points
            .iter()
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), p| {
                (lo.min(p.1), hi.max(p.1))
            });
        let mut family = Family {
            coefficients: vec![],
            along_scale,
            k_center: (k_min + k_max) / 2.0,
            k_scale: ((k_max - k_min) / 2.0).max(1.0),
        };
        let mut kept: Vec<(f32, f32, f32)> = points.to_vec();
        for _ in 0..3 {
            family.coefficients = least_squares(
                &kept
                    .iter()
                    .map(|&(a, k, c)| (family.terms(a, k), c))
                    .collect::<Vec<_>>(),
            )?;
            kept = points
                .iter()
                .copied()
                .filter(|&(a, k, c)| (family.at(a, k) - c).abs() < 0.25 * pitch)
                .collect();
            if kept.len() < 2 * (DEGREE + 1) * (DEGREE + 1) {
                return None;
            }
        }
        Some(family)
    }
}

/// The coefficients minimizing the squared error of `terms . coefficients = value`.
fn least_squares(rows: &[(Vec<f32>, f32)]) -> Option<Vec<f32>> {
    let n = rows.first()?.0.len();
    // The normal equations, in f64 for stability.
    let mut a = vec![vec![0.0f64; n + 1]; n];
    for (terms, value) in rows {
        for i in 0..n {
            for j in 0..n {
                a[i][j] += terms[i] as f64 * terms[j] as f64;
            }
            a[i][n] += terms[i] as f64 * *value as f64;
        }
    }
    // Gaussian elimination with partial pivoting.
    for col in 0..n {
        let pivot = (col..n).max_by(|&x, &y| a[x][col].abs().total_cmp(&a[y][col].abs()))?;
        if a[pivot][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, pivot);
        for row in 0..n {
            if row != col {
                let factor = a[row][col] / a[col][col];
                for k in col..=n {
                    a[row][k] -= factor * a[col][k];
                }
            }
        }
    }
    Some((0..n).map(|i| (a[i][n] / a[i][i]) as f32).collect())
}

/// The traced lines as two smooth families, and where their numbers run.
pub struct Mesh {
    horizontal: Family,
    vertical: Family,
    rows: (i64, i64),
    cols: (i64, i64),
}

impl Mesh {
    pub fn new(traced: &Traced, width: f32, height: f32) -> Option<Mesh> {
        let points = |lines: &[(i64, Curve)], axis: usize| -> Vec<(f32, f32, f32)> {
            lines
                .iter()
                .flat_map(|(k, curve)| {
                    curve.iter().map(move |&(x, y)| {
                        if axis == 0 {
                            (x, *k as f32, y)
                        } else {
                            (y, *k as f32, x)
                        }
                    })
                })
                .collect()
        };
        let span = |lines: &[(i64, Curve)]| (lines.first().unwrap().0, lines.last().unwrap().0);
        Some(Mesh {
            horizontal: Family::fit(&points(&traced.horizontal, 0), width, traced.pitch)?,
            vertical: Family::fit(&points(&traced.vertical, 1), height, traced.pitch)?,
            rows: span(&traced.horizontal),
            cols: span(&traced.vertical),
        })
    }

    /// Where horizontal line `row` crosses vertical line `col` (both may be fractional).
    fn corner(&self, row: f32, col: f32) -> (f32, f32) {
        let mut x = self.vertical.at(self.horizontal.along_scale / 2.0, col);
        let mut y = self.horizontal.at(x, row);
        for _ in 0..6 {
            x = self.vertical.at(y, col);
            y = self.horizontal.at(x, row);
        }
        (x, y)
    }

    /// The traced lines, as the mesh has them (for drawing).
    pub fn lines(&self, width: f32, height: f32) -> Vec<Curve> {
        let mut lines = vec![];
        for row in self.rows.0..=self.rows.1 {
            lines.push(
                (0..=20)
                    .map(|i| {
                        let x = i as f32 / 20.0 * width;
                        (x, self.horizontal.at(x, row as f32))
                    })
                    .collect(),
            );
        }
        for col in self.cols.0..=self.cols.1 {
            lines.push(
                (0..=20)
                    .map(|i| {
                        let y = i as f32 / 20.0 * height;
                        (self.vertical.at(y, col as f32), y)
                    })
                    .collect(),
            );
        }
        lines
    }
}

/// Sample `image` at a non-integer point.
fn sample(image: &RgbImage, x: f32, y: f32) -> Rgb<u8> {
    let (w, h) = (image.width() as i64, image.height() as i64);
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let at = |dx: i64, dy: i64| {
        let px = (x0 as i64 + dx).clamp(0, w - 1) as u32;
        let py = (y0 as i64 + dy).clamp(0, h - 1) as u32;
        *image.get_pixel(px, py)
    };
    let mut out = [0u8; 3];
    for (c, o) in out.iter_mut().enumerate() {
        let v = at(0, 0)[c] as f32 * (1.0 - fx) * (1.0 - fy)
            + at(1, 0)[c] as f32 * fx * (1.0 - fy)
            + at(0, 1)[c] as f32 * (1.0 - fx) * fy
            + at(1, 1)[c] as f32 * fx * fy;
        *o = v.round() as u8;
    }
    Rgb(out)
}

/// The picture, with each cell of the mesh mapped to a square of `size` pixels. A margin of a
/// few cells around the traced lines is carried along (the clues are often out there).
pub fn flatten(image: &RgbImage, mesh: &Mesh, size: f32) -> RgbImage {
    let margin = 6;
    let (row0, col0) = (mesh.rows.0 - margin, mesh.cols.0 - margin);
    let (rows, cols) = (
        mesh.rows.1 - mesh.rows.0 + 2 * margin,
        mesh.cols.1 - mesh.cols.0 + 2 * margin,
    );
    // The corners of each cell, worked out once.
    let corners: Vec<Vec<(f32, f32)>> = (0..=rows)
        .map(|r| {
            (0..=cols)
                .map(|c| mesh.corner((row0 + r) as f32, (col0 + c) as f32))
                .collect()
        })
        .collect();
    let (out_w, out_h) = ((cols as f32 * size) as u32, (rows as f32 * size) as u32);
    RgbImage::from_fn(out_w, out_h, |ox, oy| {
        let (u, v) = (ox as f32 / size, oy as f32 / size);
        let (c, r) = (
            (u.floor() as usize).min(cols as usize - 1),
            (v.floor() as usize).min(rows as usize - 1),
        );
        let (fu, fv) = (u - c as f32, v - r as f32);
        let (a, b) = (corners[r][c], corners[r][c + 1]);
        let (cc, d) = (corners[r + 1][c], corners[r + 1][c + 1]);
        let blend = |p: f32, q: f32, s: f32, t: f32| {
            p * (1.0 - fu) * (1.0 - fv) + q * fu * (1.0 - fv) + s * (1.0 - fu) * fv + t * fu * fv
        };
        // (Outside the picture, this repeats its edge: large blank areas throw text detection
        // off entirely.)
        sample(
            image,
            blend(a.0, b.0, cc.0, d.0),
            blend(a.1, b.1, cc.1, d.1),
        )
    })
}
