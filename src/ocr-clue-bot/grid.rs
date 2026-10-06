//! Finding the grid from its lines, rather than from the clues (which can be crossed out, or set
//! in a box per digit, or otherwise hard to read).
//!
//! A pixel on a thin line is darker (or, on a dark background, lighter) than the pixels a few
//! steps away on both sides of it. Adding that up along every row of pixels gives a profile
//! with a peak at each horizontal line; the grid's lines are evenly spaced peaks. A slight tilt
//! smears the peaks, so try a few angles, and keep the sharpest.
//!
//! Which of the evenly spaced lines bound the grid (as opposed to boxes around clues) is told by
//! how far they reach: the grid's horizontal lines run on through the row clues, if they have
//! boxes, and its vertical lines on up through the column clues, while the clue boxes' lines stop
//! at the grid. So the grid's left edge is the leftmost of the vertical lines that reach highest,
//! and its top is the topmost of the horizontal lines that reach furthest left.

use image::RgbImage;

use crate::clue_layout::{ClueLayout, Edge, Role};

/// How much each pixel looks like part of a thin horizontal line (in `response[0]`) or vertical
/// one (in `response[1]`).
pub struct Response {
    pub width: usize,
    pub height: usize,
    lines: [Vec<f32>; 2],
}

impl Response {
    pub fn new(image: &RgbImage) -> Response {
        let (width, height) = (image.width() as usize, image.height() as usize);
        let luma: Vec<f32> = image
            .pixels()
            .map(|p| 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32)
            .collect();
        let at = |x: usize, y: usize| luma[y * width + x];
        // A line is narrower than the reach, so the pixels at the reach on either side are both
        // background. (Not just one: that's the edge of something big, like a filled cell.)
        let line = |here: f32, a: f32, b: f32| -> f32 {
            let dark = a.min(b) - here;
            let light = here - a.max(b);
            dark.max(light).max(0.0)
        };
        let mut lines = [vec![0.0f32; width * height], vec![0.0f32; width * height]];
        // (Several reaches, for lines of different weights: a 4-pixel border has no response at
        // a reach of 2.)
        for reach in [2, 4, 7] {
            for y in reach..height - reach {
                for x in reach..width - reach {
                    let here = at(x, y);
                    let i = y * width + x;
                    lines[0][i] = lines[0][i].max(line(here, at(x, y - reach), at(x, y + reach)));
                    lines[1][i] = lines[1][i].max(line(here, at(x - reach, y), at(x + reach, y)));
                }
            }
        }
        Response {
            width,
            height,
            lines,
        }
    }

    /// The response for lines along `axis` (0: horizontal, 1: vertical), at a point given as
    /// `(along, across)`.
    pub fn at(&self, axis: usize, along: f32, across: f32) -> f32 {
        let (x, y) = if axis == 0 {
            (along, across)
        } else {
            (across, along)
        };
        let (x, y) = (x.round() as i64, y.round() as i64);
        if x < 0 || y < 0 || x >= self.width as i64 || y >= self.height as i64 {
            return 0.0;
        }
        self.lines[axis][y as usize * self.width + x as usize]
    }

    /// `(length along, length across)` of the image for lines along `axis`.
    fn dims(&self, axis: usize) -> (usize, usize) {
        if axis == 0 {
            (self.width, self.height)
        } else {
            (self.height, self.width)
        }
    }

    /// Total response along each line `across = c + slope * along`, for every whole `c`.
    fn profile(&self, axis: usize, slope: f32) -> Vec<f32> {
        let (len_along, len_across) = self.dims(axis);
        let mut profile = vec![0.0; len_across];
        for across in 0..len_across {
            for along in 0..len_along {
                let r = self.lines[axis][if axis == 0 {
                    across * self.width + along
                } else {
                    along * self.width + across
                }];
                if r > 0.0 {
                    let c = (across as f32 - slope * along as f32).round() as i64;
                    if c >= 0 && (c as usize) < len_across {
                        profile[c as usize] += r;
                    }
                }
            }
        }
        profile
    }
}

/// A family of evenly spaced parallel lines: `across = intercept + slope * along`.
#[derive(Clone, Debug)]
pub struct Lines {
    pub slope: f32,
    /// Where each line crosses `along = 0`, in order.
    pub intercepts: Vec<f32>,
    /// How far each line runs: `(first, last)` along it.
    pub extents: Vec<(f32, f32)>,
    pub pitch: f32,
}

impl Lines {
    pub fn at(&self, line: usize, along: f32) -> f32 {
        self.intercepts[line] + self.slope * along
    }
}

/// How well `profile` matches itself shifted by each lag, relative to no shift. (Compressed
/// first, so the heavier line every five cells doesn't drown out the rest.)
fn autocorrelation(profile: &[f32]) -> Vec<f32> {
    let compressed: Vec<f32> = profile.iter().map(|p| p.sqrt()).collect();
    let mean = compressed.iter().sum::<f32>() / compressed.len() as f32;
    let centered: Vec<f32> = compressed.iter().map(|p| p - mean).collect();
    let max_lag = profile.len() / 4;
    let scores: Vec<f32> = (0..max_lag)
        .map(|lag| {
            centered
                .iter()
                .zip(&centered[lag..])
                .map(|(a, b)| a * b)
                .sum::<f32>()
        })
        .collect();
    let zero = scores
        .first()
        .copied()
        .unwrap_or(1.0)
        .max(f32::MIN_POSITIVE);
    scores.iter().map(|s| s / zero).collect()
}

/// The cell size: the spacing that best repeats in both directions at once (cells are square,
/// or near enough for this), or a whole fraction of it, if that repeats too. (Every fifth line
/// is often heavier, which makes five cells the strongest repeat.)
fn cell_size(autocorrelations: [&[f32]; 2]) -> Option<usize> {
    let len = autocorrelations[0].len().min(autocorrelations[1].len());
    let scores: Vec<f32> = (0..len)
        .map(|lag| autocorrelations[0][lag] + autocorrelations[1][lag])
        .collect();
    let min = 8;
    let peaks: Vec<(usize, f32)> = (min.max(1)..len.saturating_sub(1))
        .filter(|&i| scores[i] > scores[i - 1] && scores[i] >= scores[i + 1] && scores[i] > 0.0)
        .map(|i| (i, scores[i]))
        .collect();
    let &(best, best_score) = peaks.iter().max_by(|a, b| a.1.total_cmp(&b.1))?;
    for k in [5, 4, 3, 2] {
        let target = best as f32 / k as f32;
        let tolerance = (0.05 * target).max(2.0);
        if let Some(&(lag, _)) = peaks
            .iter()
            .filter(|(lag, score)| {
                (*lag as f32 - target).abs() <= tolerance && *score >= 0.65 * best_score
            })
            .max_by(|a, b| a.1.total_cmp(&b.1))
        {
            return Some(lag);
        }
    }
    Some(best)
}

/// The tilt (as a slope) that makes the sharpest profile for lines along `axis`, and the profile.
fn tilt(response: &Response, axis: usize) -> (f32, Vec<f32>) {
    let sharpness = |p: &[f32]| p.iter().map(|v| v * v).sum::<f32>();
    (-12..=12)
        .map(|i| (i as f32 * 0.0044).tan()) // steps of a quarter degree
        .map(|slope| (slope, response.profile(axis, slope)))
        .max_by(|a, b| sharpness(&a.1).total_cmp(&sharpness(&b.1)))
        .unwrap()
}

/// The lines along `axis`, about `pitch` apart, if there are evenly spaced ones.
fn find_lines(
    response: &Response,
    axis: usize,
    (slope, profile): (f32, Vec<f32>),
    pitch: usize,
) -> Option<Lines> {
    let pitch = pitch as f32;
    // The comb that catches the most: where the lines are.
    let near_max = |c: f32| -> (f32, f32) {
        let lo = (c - 0.2 * pitch).max(0.0) as usize;
        let hi = ((c + 0.2 * pitch) as usize).min(profile.len() - 1);
        (lo..=hi)
            .map(|i| (i as f32, profile[i]))
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .unwrap_or((c, 0.0))
    };
    let phase = (0..pitch as usize)
        .max_by(|&a, &b| {
            let comb = |offset: usize| -> f32 {
                (offset..profile.len())
                    .step_by(pitch as usize)
                    .map(|c| profile[c])
                    .sum()
            };
            comb(a).total_cmp(&comb(b))
        })?
        .to_owned() as f32;

    // Each tooth of the comb that lands on a strong peak is a line. Then fit the spacing to
    // those exactly: it needn't be a whole number of pixels.
    let teeth: Vec<(f32, f32)> = (0..)
        .map(|k| phase + k as f32 * pitch)
        .take_while(|&c| c < profile.len() as f32)
        .map(near_max)
        .collect();
    let mut strengths: Vec<f32> = teeth.iter().map(|t| t.1).collect();
    strengths.sort_by(f32::total_cmp);
    let reference = strengths[strengths.len() * 3 / 4];
    let strong: Vec<(f32, f32)> = teeth
        .iter()
        .enumerate()
        .filter(|(_, t)| t.1 > 0.35 * reference)
        .map(|(k, t)| (k as f32, t.0))
        .collect();
    if strong.len() < 3 {
        return None;
    }
    let n = strong.len() as f32;
    let (mk, mc) = (
        strong.iter().map(|s| s.0).sum::<f32>() / n,
        strong.iter().map(|s| s.1).sum::<f32>() / n,
    );
    let pitch = strong.iter().map(|s| (s.0 - mk) * (s.1 - mc)).sum::<f32>()
        / strong.iter().map(|s| (s.0 - mk).powi(2)).sum::<f32>();
    let start = mc - pitch * mk;

    // Every line from the first strong one to the last (a weak one in between is just faint, or
    // hidden), and how far each runs.
    let (len_along, _) = response.dims(axis);
    let (first_k, last_k) = (strong[0].0 as usize, strong[strong.len() - 1].0 as usize);
    let mut intercepts = vec![];
    let mut extents = vec![];
    for k in first_k..=last_k {
        let c = start + k as f32 * pitch;
        let along_line = |along: f32| -> f32 {
            let across = c + slope * along;
            (-1..=1)
                .map(|d| response.at(axis, along, across + d as f32))
                .fold(0.0, f32::max)
        };
        // Over each stretch a cell long: is most of it line?
        let threshold = {
            let mut values: Vec<f32> = (0..len_along).map(|a| along_line(a as f32)).collect();
            values.sort_by(f32::total_cmp);
            0.3 * values[values.len() * 95 / 100]
        };
        let cells = (len_along as f32 / pitch) as usize;
        let on: Vec<bool> = (0..cells)
            .map(|i| {
                let from = (i as f32 * pitch) as usize;
                let to = (((i + 1) as f32 * pitch) as usize).min(len_along);
                let count = (from..to)
                    .filter(|&a| along_line(a as f32) > threshold)
                    .count();
                count as f32 > 0.5 * (to - from) as f32
            })
            .collect();
        // The longest stretch, bridging gaps of a few cells (where filled cells hide the line).
        let mut best: Option<(usize, usize)> = None;
        let mut current: Option<(usize, usize)> = None;
        let mut gap = 0;
        for (i, &on) in on.iter().enumerate() {
            if on {
                current = Some(match current {
                    Some((s, _)) if gap <= 3 => (s, i),
                    _ => (i, i),
                });
                gap = 0;
                if best.is_none_or(|b| current.unwrap().1 - current.unwrap().0 > b.1 - b.0) {
                    best = current;
                }
            } else {
                gap += 1;
            }
        }
        // (To the pixel, within the end cells.)
        let (first, last) = best.map_or((f32::INFINITY, f32::NEG_INFINITY), |(a, b)| {
            let (a, b) = (
                (a as f32 * pitch) as usize,
                ((b + 1) as f32 * pitch) as usize,
            );
            let inside = |along: &usize| along_line(*along as f32) > threshold;
            let first = (a..(a + pitch as usize).min(len_along))
                .find(inside)
                .unwrap_or(a);
            let last = (b.saturating_sub(pitch as usize)..b.min(len_along))
                .rev()
                .find(inside)
                .unwrap_or(b);
            (first as f32, last as f32)
        });
        intercepts.push(c);
        extents.push((first, last));
    }
    Some(Lines {
        slope,
        intercepts,
        extents,
        pitch,
    })
}

/// A grid found from its lines.
#[derive(Clone, Debug)]
pub struct Grid {
    /// All the evenly spaced lines found, horizontal and vertical.
    pub horizontal: Lines,
    pub vertical: Lines,
    /// Which of those bound the grid: `(first, last)` indices.
    pub rows: (usize, usize),
    pub cols: (usize, usize),
}

/// The size of a cell, roughly (and whatever tilt makes the lines sharpest, in each direction).
fn rough_cell_size(response: &Response) -> Option<(usize, (f32, Vec<f32>), (f32, Vec<f32>))> {
    let (h_tilt, v_tilt) = (tilt(response, 0), tilt(response, 1));
    let pitch = cell_size([&autocorrelation(&h_tilt.1), &autocorrelation(&v_tilt.1)])?;
    Some((pitch, h_tilt, v_tilt))
}

/// The size of a cell, roughly, as found from the lines.
pub fn rough_pitch(response: &Response) -> Option<f32> {
    rough_cell_size(response).map(|(pitch, _, _)| pitch as f32)
}

/// The grid, from its lines. `cell` is the size of a cell, if it's known already: the lines
/// alone can be misleading about that (in a printed puzzle, digits and marks in the cells repeat
/// at half a cell, too).
pub fn find(image: &RgbImage, cell: Option<f32>) -> Option<Grid> {
    let response = Response::new(image);
    let (pitch, h_tilt, v_tilt) = match cell {
        Some(cell) => (
            cell.round() as usize,
            tilt(&response, 0),
            tilt(&response, 1),
        ),
        None => rough_cell_size(&response)?,
    };
    let horizontal = find_lines(&response, 0, h_tilt, pitch)?;
    let vertical = find_lines(&response, 1, v_tilt, pitch)?;

    // The grid and any clue boxes are a run of long lines, side by side. (Short ones that happen
    // to fall in step are bits of the rest of the picture.)
    let long = |lines: &Lines| -> Vec<bool> {
        let length = |i: usize| lines.extents[i].1 - lines.extents[i].0;
        let longest = (0..lines.intercepts.len()).map(length).fold(0.0, f32::max);
        (0..lines.intercepts.len())
            .map(|i| length(i) >= 0.5 * longest)
            .collect()
    };
    let run = |lines: &Lines| -> Option<(usize, usize)> {
        let long = long(lines);
        let mut best: Option<(usize, usize)> = None;
        let mut start = None;
        for i in 0..=long.len() {
            match (i < long.len() && long[i], start) {
                (true, None) => start = Some(i),
                (false, Some(s)) => {
                    if best.is_none_or(|(a, b)| i - 1 - s > b - a) {
                        best = Some((s, i - 1));
                    }
                    start = None;
                }
                _ => {}
            }
        }
        best
    };
    // Where the long lines that *don't* reach furthest start. Those are clue boxes' lines, and
    // they start at the edge of the grid. (That's more reliable than the grid's own border,
    // which disappears next to a filled cell.)
    let clue_box_start = |lines: &Lines| -> Option<f32> {
        let long = long(lines);
        let furthest = (0..long.len())
            .filter(|&i| long[i])
            .map(|i| lines.extents[i].0)
            .fold(f32::INFINITY, f32::min);
        let mut starts: Vec<f32> = (0..long.len())
            .filter(|&i| long[i] && lines.extents[i].0 > furthest + 1.5 * lines.pitch)
            .map(|i| lines.extents[i].0)
            .collect();
        starts.sort_by(f32::total_cmp);
        starts.get(starts.len() / 2).copied()
    };
    // The line nearest `across` (at `along`).
    let nearest = |lines: &Lines, across: f32, along: f32| -> usize {
        (0..lines.intercepts.len())
            .min_by(|&a, &b| {
                (lines.at(a, along) - across)
                    .abs()
                    .total_cmp(&(lines.at(b, along) - across).abs())
            })
            .unwrap()
    };
    // Otherwise, the grid's lines reach furthest: horizontal ones left, vertical ones up.
    // (Within a cell or so: the ends of some lines are lost.)
    let reaching = |lines: &Lines, (first, last): (usize, usize)| -> Option<usize> {
        let furthest = (first..=last)
            .map(|i| lines.extents[i].0)
            .fold(f32::INFINITY, f32::min);
        (first..=last).find(|&i| lines.extents[i].0 <= furthest + 1.5 * lines.pitch)
    };
    let (rows_run, cols_run) = (run(&horizontal)?, run(&vertical)?);
    let middle = |lines: &Lines, (first, last): (usize, usize)| {
        (lines.intercepts[first] + lines.intercepts[last]) / 2.0
    };
    let top = match clue_box_start(&vertical) {
        Some(y) => nearest(&horizontal, y, middle(&vertical, cols_run)),
        None => reaching(&horizontal, rows_run)?,
    };
    let left = match clue_box_start(&horizontal) {
        Some(x) => nearest(&vertical, x, middle(&horizontal, rows_run)),
        None => reaching(&vertical, cols_run)?,
    };
    // And the grid runs to the last line (clues at the bottom or right aren't handled).
    let (bottom, right) = (rows_run.1, cols_run.1);
    if bottom <= top || right <= left {
        return None;
    }
    Some(Grid {
        horizontal,
        vertical,
        rows: (top, bottom),
        cols: (left, right),
    })
}

impl Grid {
    pub fn width(&self) -> usize {
        self.cols.1 - self.cols.0
    }

    pub fn height(&self) -> usize {
        self.rows.1 - self.rows.0
    }

    /// Move `layout`'s lanes to the middles of the cells between this grid's lines, where they're
    /// close (the lines are a more exact guide than the clues). (Not its edges: starting a lane's
    /// strip right at a heavy border invites reading the border as a "1".)
    pub fn snap(&self, layout: &mut ClueLayout) {
        let (h, v) = (&self.horizontal, &self.vertical);
        let snap = |centers: &mut Vec<f32>, lines: &Lines, at: f32, pitch: f32| {
            let middles: Vec<f32> = (0..lines.intercepts.len().saturating_sub(1))
                .map(|i| (lines.at(i, at) + lines.at(i + 1, at)) / 2.0)
                .collect();
            for center in centers.iter_mut() {
                if let Some(&m) = middles
                    .iter()
                    .min_by(|a, b| (*a - *center).abs().total_cmp(&(*b - *center).abs()))
                    && (m - *center).abs() < 0.4 * pitch
                {
                    *center = m;
                }
            }
        };
        // (Along the edges, where the clues are.)
        let top = layout
            .grid_top
            .at(layout.col_centers.first().copied().unwrap_or(0.0));
        let left = layout
            .grid_left
            .at(layout.row_centers.first().copied().unwrap_or(0.0));
        snap(&mut layout.col_centers, v, top, layout.col_pitch);
        snap(&mut layout.row_centers, h, left, layout.row_pitch);
    }

    /// A layout with the grid's geometry, but no clues yet. `glyph_height` is the size of the
    /// clues' digits; `glyphs`, how many digits the layout is for.
    pub fn layout(&self, glyph_height: f32, glyphs: usize) -> ClueLayout {
        let (h, v) = (&self.horizontal, &self.vertical);
        let grid_top = Edge {
            intercept: h.intercepts[self.rows.0],
            slope: h.slope,
        };
        let grid_left = Edge {
            intercept: v.intercepts[self.cols.0],
            slope: v.slope,
        };
        // Along the edges, where the clues are.
        let middle_x = (v.intercepts[self.cols.0] + v.intercepts[self.cols.1]) / 2.0;
        let middle_y = (h.intercepts[self.rows.0] + h.intercepts[self.rows.1]) / 2.0;
        let top_y = grid_top.at(middle_x);
        let left_x = grid_left.at(middle_y);
        ClueLayout {
            rows: vec![vec![]; self.height()],
            cols: vec![vec![]; self.width()],
            roles: vec![Role::Ignored; glyphs],
            grid_top,
            grid_left,
            col_centers: (self.cols.0..self.cols.1)
                .map(|i| (v.at(i, top_y) + v.at(i + 1, top_y)) / 2.0)
                .collect(),
            row_centers: (self.rows.0..self.rows.1)
                .map(|i| (h.at(i, left_x) + h.at(i + 1, left_x)) / 2.0)
                .collect(),
            col_pitch: v.pitch,
            row_pitch: h.pitch,
            glyph_height,
            edges_on_lines: true,
            warnings: vec![],
        }
    }
}
