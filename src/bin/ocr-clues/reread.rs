//! Phase two: once `clue_layout` has found the grid, look for each lane's clues right where they
//! must be, instead of trusting the first pass's detection.
//!
//! The pixels say where each number is: a column's clues are lines of ink stacked above the
//! column, and a row's clues are runs of ink left of the row, split into numbers where the gaps
//! are wide. (Measured on the ink itself, the space between two numbers is reliably wider than
//! the space between the digits of one, which OCR's character boxes are too rough to show.) Each
//! number is then recognized as a crop of its own.

use crate::clue_layout::{BLOTTED, ClueLayout, split_threshold};
use image::RgbImage;
use ocrs::{OcrEngine, OcrInput};
use rten_imageproc::{RectF, RotatedRect};

use crate::{as_digit, templates};

/// A box in image pixels: `(left, top, right, bottom)`, inclusive.
pub type Area = (i64, i64, i64, i64);

/// A number found in phase two, for the debug image.
pub struct Found {
    pub area: Area,
    /// What it read as, or `None` if it wasn't a number.
    pub number: Option<u16>,
    /// What recognition said, if comparing with the other digits changed it.
    pub recognized: Option<u16>,
}

pub struct Reread {
    pub cols: Vec<Vec<u16>>,
    pub rows: Vec<Vec<u16>>,
    /// Lanes where phase two failed, so phase one's reading stands.
    pub fallbacks: usize,
    /// Numbers changed by comparing digits (see `templates`).
    pub corrections: usize,
    /// Clues that are there, but couldn't be read.
    pub blots: usize,
    pub found: Vec<Found>,
}

/// The image, in shades of gray.
pub struct Luma {
    luma: Vec<f32>,
    width: i64,
    height: i64,
}

impl Luma {
    fn new(image: &RgbImage) -> Luma {
        let luma = image
            .pixels()
            .map(|p| 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32)
            .collect();
        Luma {
            luma,
            width: image.width() as i64,
            height: image.height() as i64,
        }
    }

    pub fn at(&self, x: i64, y: i64) -> Option<f32> {
        (x >= 0 && y >= 0 && x < self.width && y < self.height)
            .then(|| self.luma[(y * self.width + x) as usize])
    }
}

/// Which pixels are ink, in a strip of the image running along one lane: anything far enough
/// from the background. That works for dark text on light and light on dark, and for grayed-out
/// clues. The background can shade from one end of the strip to the other, so it's measured
/// separately at each step along it, from the lane's own pixels nearby. (Not its neighbors':
/// some apps highlight the lane under the cursor.)
struct Strip {
    /// `background[i]` is for step `i` along the strip.
    background: Vec<f32>,
    threshold: f32,
}

impl Strip {
    /// `lane(i)` lists the pixels across the lane at step `i`. `reach` is how many steps either
    /// side to take the background from; it should span the gaps between digits.
    fn new(steps: usize, lane: impl Fn(usize) -> Vec<f32>, reach: usize) -> Strip {
        let across: Vec<Vec<f32>> = (0..steps).map(lane).collect();
        let background: Vec<f32> = (0..steps)
            .map(|i| {
                // (Every other step is plenty, and quicker.)
                let mut values: Vec<f32> = (i.saturating_sub(reach)..(i + reach + 1).min(steps))
                    .step_by(2)
                    .flat_map(|j| across[j].iter().copied())
                    .collect();
                values.sort_by(f32::total_cmp);
                values.get(values.len() / 2).copied().unwrap_or(0.0)
            })
            .collect();
        let mut contrast: Vec<f32> = (0..steps)
            .flat_map(|i| {
                across[i]
                    .iter()
                    .map(|v| (v - background[i]).abs())
                    .collect::<Vec<_>>()
            })
            .collect();
        contrast.sort_by(f32::total_cmp);
        let strong = contrast
            .get(contrast.len() * 98 / 100)
            .copied()
            .unwrap_or(0.0);
        Strip {
            background,
            // Grayed-out clues can have a third of the contrast of the others.
            threshold: (0.3 * strong).max(20.0),
        }
    }

    fn is_ink(&self, step: usize, value: Option<f32>) -> bool {
        value.is_some_and(|v| (v - self.background[step]).abs() > self.threshold)
    }
}

/// Runs of `true` in `profile`, as inclusive index ranges, joining runs less than `gap` apart (a
/// digit broken up by antialiasing, say).
fn runs(profile: &[bool], gap: f32) -> Vec<(usize, usize)> {
    let mut runs: Vec<(usize, usize)> = vec![];
    let mut start = None;
    for (i, &on) in profile.iter().chain([&false]).enumerate() {
        match (on, start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                match runs.last_mut() {
                    Some(last) if ((s - last.1 - 1) as f32) < gap => last.1 = i - 1,
                    _ => runs.push((s, i - 1)),
                }
                start = None;
            }
            _ => {}
        }
    }
    runs
}

/// A lane's clues, before recognition: each number's area. `None` if it doesn't look like clues.
type Lane = Option<Vec<Area>>;

/// Where a lane's strip starts, relative to the edge of the grid: just outside the grid's
/// border, if that's what the edge is; otherwise, a little past where the clues seem to end
/// (in case they reach a little further).
fn edge_margin(layout: &ClueLayout) -> f32 {
    if layout.edges_on_lines {
        -(0.2 * layout.glyph_height).max(3.0)
    } else {
        0.25 * layout.glyph_height
    }
}

/// The strip of the picture where one lane's clues are: above a column, or left of a row.
/// Positions in it are `(step, across)`: `step` counts outward from the grid, and `across` is
/// the image coordinate across the lane.
struct LaneStrip<'a> {
    luma: &'a Luma,
    strip: Strip,
    vertical: bool,
    /// The image coordinate of step 0.
    base: i64,
    steps: usize,
    /// The lane's extent across, inclusive.
    across: (i64, i64),
    /// Positions across the lane (relative to its start) taken up by a line along it: a box's
    /// side, or the divider between two lanes' clues. Those aren't ink, for these purposes.
    ruled: Vec<bool>,
}

impl LaneStrip<'_> {
    /// The strip above the column centered at `center`.
    fn column<'a>(luma: &'a Luma, layout: &ClueLayout, center: f32) -> LaneStrip<'a> {
        let h = layout.glyph_height;
        let half = 0.45 * layout.col_pitch;
        let across = ((center - half) as i64, (center + half) as i64);
        let base = (layout.grid_top.at(center) + edge_margin(layout)) as i64;
        let steps = (base as f32).min(12.0 * h) as usize + 1;
        LaneStrip::new(luma, true, base, steps, across, h)
    }

    /// The strip left of the row centered at `center`.
    fn row<'a>(luma: &'a Luma, layout: &ClueLayout, center: f32) -> LaneStrip<'a> {
        let h = layout.glyph_height;
        let half = 0.42 * layout.row_pitch;
        let across = ((center - half) as i64, (center + half) as i64);
        let base = (layout.grid_left.at(center) + edge_margin(layout)) as i64;
        let steps = (base as f32).min(25.0 * h) as usize + 1;
        LaneStrip::new(luma, false, base, steps, across, h)
    }

    fn new(
        luma: &Luma,
        vertical: bool,
        base: i64,
        steps: usize,
        across: (i64, i64),
        h: f32,
    ) -> LaneStrip<'_> {
        let point = |i: usize, a: i64| {
            if vertical {
                (a, base - i as i64)
            } else {
                (base - i as i64, a)
            }
        };
        let strip = Strip::new(
            steps,
            |i| {
                (across.0..=across.1)
                    .filter_map(|a| {
                        let (x, y) = point(i, a);
                        luma.at(x, y)
                    })
                    .collect()
            },
            (1.2 * h) as usize,
        );
        let mut lane = LaneStrip {
            luma,
            strip,
            vertical,
            base,
            steps,
            across,
            ruled: vec![false; (across.1 - across.0 + 1) as usize],
        };
        // A line along the lane is ink for much longer than any clue is: longer than a digit is
        // wide, along a row; or, along a column, than a few digits stacked tightly enough to
        // touch ("1" on "1", stroke on stroke). (Allowing for the line drifting by a pixel.)
        let longer_than = if vertical { 3.0 * h } else { 1.5 * h };
        let inky = |i: usize, a: i64| {
            (a - 1..=a + 1).any(|a| {
                let (x, y) = lane.point(i, a);
                lane.strip.is_ink(i, luma.at(x, y))
            })
        };
        lane.ruled = (across.0..=across.1)
            .map(|a| {
                let mut longest = 0;
                let mut run = 0;
                for i in 0..steps {
                    if inky(i, a) {
                        run += 1;
                        longest = longest.max(run);
                    } else {
                        run = 0;
                    }
                }
                longest as f32 > longer_than
            })
            .collect();
        lane
    }

    fn point(&self, step: usize, across: i64) -> (i64, i64) {
        if self.vertical {
            (across, self.base - step as i64)
        } else {
            (self.base - step as i64, across)
        }
    }

    fn is_ink(&self, step: usize, across: i64) -> bool {
        if self.ruled[(across - self.across.0) as usize] {
            return false;
        }
        let (x, y) = self.point(step, across);
        self.strip.is_ink(step, self.luma.at(x, y))
    }

    /// How much ink there is at each step, leaving out rules across the whole lane (the borders
    /// of boxes, and the like).
    fn ink(&self) -> Vec<usize> {
        let width = (self.across.1 - self.across.0 + 1) as f32;
        (0..self.steps)
            .map(|i| {
                let count = (self.across.0..=self.across.1)
                    .filter(|&a| self.is_ink(i, a))
                    .count();
                if count as f32 >= 0.85 * width {
                    0
                } else {
                    count
                }
            })
            .collect()
    }

    /// Where across the lane there's ink, between steps `from` and `to`.
    fn across_ink(&self, from: usize, to: usize) -> Vec<bool> {
        (self.across.0..=self.across.1)
            .map(|a| (from..=to).any(|i| self.is_ink(i, a)))
            .collect()
    }

    /// The image area of steps `from..=to`, and `across` (relative to the lane's start) from
    /// `a0..=a1`.
    fn area(&self, from: usize, to: usize, (a0, a1): (usize, usize)) -> Area {
        let (x0, y0) = self.point(to, self.across.0 + a0 as i64);
        let (x1, y1) = self.point(from, self.across.0 + a1 as i64);
        (x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1))
    }
}

/// The clues of the column centered at `center`, from the top.
fn column(luma: &Luma, layout: &ClueLayout, center: f32) -> Lane {
    let h = layout.glyph_height;
    let strip = LaneStrip::column(luma, layout, center);
    let ink = strip.ink();
    let profile: Vec<bool> = ink.iter().map(|&c| c > 0).collect();
    let mut lines = vec![];
    let mut last_end = 0;
    for (start, end) in runs(&profile, 0.1 * h) {
        // A big gap is the end of the clues (or there were none).
        if start as f32 - last_end as f32 > 1.5 * h {
            break;
        }
        last_end = end;
        let tall = (end - start + 1) as f32;
        if tall < 0.3 * h || ((start as f32) < 0.33 * h && tall < 0.25 * h) {
            continue; // a speck, or what's left of the grid's border
        }
        if tall > 1.6 * h {
            // Clues stacked so tightly that they touch ("1" on "1" on "1", stroke on stroke).
            lines.extend(split_stack(&ink, start, end, h));
        } else {
            lines.push((start, end));
        }
    }
    let mut areas = vec![];
    for (start, end) in lines {
        // Across the line, the ink nearest the middle, plus anything close enough to be another
        // digit of the same number. (Not ink at the edges, from a neighbor's two-digit number.)
        let mid = (center - strip.across.0 as f32) as usize;
        let mut pieces = runs(&strip.across_ink(start, end), 0.4 * h);
        pieces.sort_by_key(|&(a, b)| {
            if a <= mid && mid <= b {
                0
            } else {
                a.abs_diff(mid).min(b.abs_diff(mid))
            }
        });
        areas.push(strip.area(start, end, *pieces.first()?));
    }
    areas.reverse();
    Some(areas)
}

/// Cut a run of steps `start..=end` that's too tall for one digit into pieces about a digit
/// tall, where there's least ink.
fn split_stack(ink: &[usize], start: usize, end: usize, h: f32) -> Vec<(usize, usize)> {
    let tall = (end - start + 1) as f32;
    // Digits are a little less than `h` tall (the height of OCR's boxes), with a little space.
    let pieces = ((tall + 0.3 * h) / (1.1 * h)).round().max(2.0) as usize;
    let step = tall / pieces as f32;
    let mut cuts = vec![start];
    for i in 1..pieces {
        let ideal = start as f32 + i as f32 * step;
        let window = (ideal - 0.35 * step) as usize..=(ideal + 0.35 * step) as usize;
        cuts.push(window.min_by_key(|&j| ink[j]).unwrap());
    }
    cuts.push(end + 1);
    cuts.windows(2)
        .filter_map(|w| {
            // Trimmed to the ink.
            let inky: Vec<usize> = (w[0]..w[1]).filter(|&j| ink[j] > 0).collect();
            Some((*inky.first()?, *inky.last()?))
        })
        .collect()
}

/// The clues of the row centered at `center`, as the areas of the individual digits (or other
/// marks), from the left.
fn row_marks(luma: &Luma, layout: &ClueLayout, center: f32) -> Lane {
    let h = layout.glyph_height;
    let strip = LaneStrip::row(luma, layout, center);
    let profile: Vec<bool> = strip.ink().iter().map(|&c| c > 0).collect();
    let mut marks = vec![];
    let mut last_end = 0;
    for (start, end) in runs(&profile, 0.1 * h) {
        if marks.is_empty() && start as f32 > 1.6 * h {
            return Some(vec![]); // nothing next to the grid
        }
        if start as f32 - last_end as f32 > 2.2 * h {
            break;
        }
        if (end - start + 1) as f32 > 1.5 * h {
            return None; // too wide to be a digit
        }
        let across = strip.across_ink(start, end);
        let (Some(a0), Some(a1)) = (
            across.iter().position(|&a| a),
            across.iter().rposition(|&a| a),
        ) else {
            continue;
        };
        if ((a1 - a0 + 1) as f32) < 0.3 * h {
            continue; // a speck, or a strikethrough's stray end
        }
        if (start as f32) < 0.33 * h && ((end - start + 1) as f32) < 0.25 * h {
            continue; // what's left of the grid's border
        }
        marks.push(strip.area(start, end, (a0, a1)));
        last_end = end;
    }
    marks.reverse();
    Some(marks)
}

/// Where the clues sit along the lanes, if they're on a regular spacing (each in its own box, or
/// in slots), as `(first, spacing)` in steps from the grid. The clues of every lane are at the
/// same places, so this is from all of them at once.
fn slots(strips: &[LaneStrip], h: f32) -> Option<(f32, f32)> {
    let steps = strips.iter().map(|s| s.steps).min()?;
    let mut total = vec![0.0; steps];
    for strip in strips {
        for (t, c) in total.iter_mut().zip(strip.ink()) {
            *t += c as f32;
        }
    }
    let mean = total.iter().sum::<f32>() / steps as f32;
    let centered: Vec<f32> = total.iter().map(|t| t - mean).collect();
    let score = |lag: usize| -> f32 {
        centered
            .iter()
            .zip(&centered[lag.min(steps)..])
            .map(|(a, b)| a * b)
            .sum()
    };
    let zero = score(0).max(f32::MIN_POSITIVE);
    let (lo, hi) = ((0.8 * h) as usize, ((3.0 * h) as usize).min(steps / 2));
    let lags: Vec<(usize, f32)> = (lo.max(1)..=hi)
        .map(|lag| (lag, score(lag) / zero))
        .collect();
    let peaks: Vec<(usize, f32)> = (1..lags.len().saturating_sub(1))
        .filter(|&i| lags[i].1 > lags[i - 1].1 && lags[i].1 >= lags[i + 1].1)
        .map(|i| lags[i])
        .collect();
    let &(spacing, strength) = peaks.iter().max_by(|a, b| a.1.total_cmp(&b.1))?;
    // Not regular enough to go by.
    if strength < 0.3 {
        return None;
    }
    let spacing = spacing as f32;
    // The first slot: where a comb of that spacing catches the most ink.
    let first = (0..spacing as usize).max_by(|&a, &b| {
        let comb = |o: usize| -> f32 {
            (0..)
                .map(|k| o as f32 + k as f32 * spacing)
                .take_while(|&i| (i as usize) < steps)
                .map(|i| total[i as usize])
                .sum()
        };
        comb(a).total_cmp(&comb(b))
    })? as f32;
    Some((first, spacing))
}

/// A lane's clues by slot: the ink in each slot from the grid out, until an empty one.
fn by_slot(strip: &LaneStrip, (first, spacing): (f32, f32), h: f32) -> Vec<Area> {
    let ink = strip.ink();
    let mut areas = vec![];
    for k in 0.. {
        let center = first + k as f32 * spacing;
        let from = (center - 0.45 * spacing).max(0.0) as usize;
        let to = (center + 0.45 * spacing) as usize;
        if to >= strip.steps {
            break;
        }
        // Enough ink to be a digit, not a speck.
        let amount: usize = ink[from..=to].iter().sum();
        if (amount as f32) < 0.5 * h {
            break;
        }
        let across = strip.across_ink(from, to);
        let (Some(a0), Some(a1)) = (
            across.iter().position(|&a| a),
            across.iter().rposition(|&a| a),
        ) else {
            break;
        };
        let steps: Vec<usize> = (from..=to).filter(|&i| ink[i] > 0).collect();
        let (s0, s1) = (steps[0], steps[steps.len() - 1]);
        areas.push(strip.area(s0, s1, (a0, a1)));
    }
    areas.reverse();
    areas
}

/// The threshold that best separates `distances` into two groups (Otsu's method), the digits of
/// one number and neighboring numbers, and how cleanly: the difference between the groups'
/// averages against how much they vary within themselves. `None` if there aren't two groups
/// with averages at least `ratio` apart, or the closer group isn't close enough (`limit`) to be
/// digits of one number.
/// How cleanly `threshold` splits `distances`: the difference between the two groups' averages,
/// against how much they vary within themselves.
fn clarity(distances: &[f32], threshold: f32) -> f32 {
    let (lo, hi): (Vec<f32>, Vec<f32>) = distances.iter().partition(|&&d| d < threshold);
    if lo.is_empty() || hi.is_empty() {
        return 0.0;
    }
    let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len() as f32;
    let variance =
        |v: &[f32], m: f32| v.iter().map(|x| (x - m).powi(2)).sum::<f32>() / v.len() as f32;
    let (m0, m1) = (mean(&lo), mean(&hi));
    (m1 - m0).powi(2) / (variance(&lo, m0) + variance(&hi, m1)).max(1.0)
}

fn split_otsu(mut distances: Vec<f32>, limit: f32, ratio: f32) -> Option<(f32, f32)> {
    distances.sort_by(f32::total_cmp);
    let n = distances.len();
    let mut best: Option<(f32, f32)> = None; // (separation, threshold)
    for i in 1..n {
        if distances[i] - distances[i - 1] <= 0.0 {
            continue;
        }
        let (lo, hi) = distances.split_at(i);
        let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len() as f32;
        let (m0, m1) = (mean(lo), mean(hi));
        if m1 < ratio * m0.max(1.0) || m0 > limit {
            continue;
        }
        let separation = (lo.len() * hi.len()) as f32 * (m1 - m0).powi(2);
        if best.is_none_or(|(s, _)| separation > s) {
            best = Some((
                separation,
                ((distances[i - 1] + distances[i]) / 2.0).min(limit),
            ));
        }
    }
    best.map(|(_, threshold)| (threshold, clarity(&distances, threshold)))
}

fn union(a: Area, b: Area) -> Area {
    (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3))
}

/// The area to hand to recognition: a little margin, and not too narrow (a lone "1" is).
fn crop((left, top, right, bottom): Area) -> RotatedRect {
    let (top, bottom) = (top as f32 - 2.0, bottom as f32 + 2.0);
    let center = (left + right) as f32 / 2.0;
    let half_width = ((right - left) as f32 / 2.0 + 2.0).max((bottom - top) * 0.35);
    RotatedRect::from_rect(RectF::from_tlbr(
        top,
        center - half_width,
        bottom,
        center + half_width,
    ))
}

/// Recognition's reading of a number, if it's one or two digits.
fn as_number(text: &str) -> Option<u16> {
    let digits: Vec<u8> = text
        .chars()
        .filter(|c| *c != ' ')
        .map(as_digit)
        .collect::<Option<_>>()?;
    (1..=2)
        .contains(&digits.len())
        .then(|| digits.iter().fold(0, |n, &d| n * 10 + d as u16))
}

pub fn reread(
    engine: &OcrEngine,
    input: &OcrInput,
    image: &RgbImage,
    layout: &ClueLayout,
    compare_digits: bool,
    by_slots: bool,
) -> anyhow::Result<Reread> {
    if layout.col_centers.is_empty() || layout.row_centers.is_empty() {
        anyhow::bail!("the grid has no cells");
    }
    let luma = Luma::new(image);
    let h = layout.glyph_height;

    // Phase one can miss a lane at the far end, so look one further, too. (If there's nothing
    // that reads as clues there, it's dropped.)
    let col_center = |c: usize| layout.col_centers[0] + c as f32 * layout.col_pitch;
    let row_center = |r: usize| layout.row_centers[0] + r as f32 * layout.row_pitch;
    let (width, height) = (layout.col_centers.len(), layout.row_centers.len());
    let cols: Vec<Lane> = (0..=width)
        .map(|c| column(&luma, layout, col_center(c)))
        .collect();
    let row_marks: Vec<Lane> = (0..=height)
        .map(|r| row_marks(&luma, layout, row_center(r)))
        .collect();

    // Group a row's marks into numbers, by the distances between them. All the rows share a
    // typeface, so decide what's far apart from all of them together. But what to measure
    // depends on the typeface: in most, it's the space between digits that's the same everywhere
    // (a "1" is narrow); in some, every digit takes the same width (a "1" sits in a lot of
    // space), and it's the distance from one digit's middle to the next's. Whichever splits more
    // cleanly into near and far is the one to go by.
    let gap = |a: &Area, b: &Area| (b.0 - a.2 - 1) as f32;
    let advance = |a: &Area, b: &Area| (b.0 + b.2 - a.0 - a.2) as f32 / 2.0;
    let measured = |distance: &dyn Fn(&Area, &Area) -> f32| -> Vec<f32> {
        row_marks
            .iter()
            .flatten()
            .flat_map(|marks| marks.windows(2).map(|w| distance(&w[0], &w[1])))
            .collect()
    };
    let gaps = measured(&gap);
    let by_gap = match split_threshold(gaps.clone(), 0.45 * h, 1.5) {
        0.0 => None,
        t => Some((t, clarity(&gaps, t))),
    };
    let by_advance = split_otsu(measured(&advance), 1.0 * h, 1.1);
    let (distance, threshold): (&dyn Fn(&Area, &Area) -> f32, f32) = match (by_gap, by_advance) {
        // From middle to middle, every digit is about as far from the next as any other: they're
        // all numbers of their own.
        (_, None) => (&gap, 0.0),
        (Some(g), Some(a)) if g.1 >= a.1 => (&gap, g.0),
        (_, Some(a)) => (&advance, a.0),
    };
    let rows: Vec<Lane> = row_marks
        .into_iter()
        .map(|marks| {
            let mut numbers: Vec<Area> = vec![];
            let mut previous: Option<Area> = None;
            for mark in marks? {
                match (numbers.last_mut(), previous) {
                    (Some(last), Some(previous)) if distance(&previous, &mark) < threshold => {
                        *last = union(*last, mark)
                    }
                    _ => numbers.push(mark),
                }
                previous = Some(mark);
            }
            Some(numbers)
        })
        .collect();

    // Where a lane's clues can't all be read as ink runs (they're crossed out, or run together),
    // try going by slots instead, if the clues are on a regular spacing. (Only where the caller
    // says they might be: in a picture whose clues aren't so neatly arranged, the background
    // can look like slots full of ink.)
    let col_strips: Vec<LaneStrip> = (0..width)
        .map(|c| LaneStrip::column(&luma, layout, col_center(c)))
        .collect();
    let row_strips: Vec<LaneStrip> = (0..height)
        .map(|r| LaneStrip::row(&luma, layout, row_center(r)))
        .collect();
    let (col_slots, row_slots) = if by_slots {
        (slots(&col_strips, h), slots(&row_strips, h))
    } else {
        (None, None)
    };
    let slotted = |strips: &[LaneStrip], slots: Option<(f32, f32)>| -> Vec<Option<Vec<Area>>> {
        strips
            .iter()
            .map(|strip| slots.map(|slots| by_slot(strip, slots, h)))
            .collect()
    };
    let col_slotted = slotted(&col_strips, col_slots);
    let row_slotted = slotted(&row_strips, row_slots);

    // Recognize every number, both ways, at once.
    let areas: Vec<Area> = cols
        .iter()
        .chain(&rows)
        .chain(&col_slotted)
        .chain(&row_slotted)
        .flatten()
        .flatten()
        .copied()
        .collect();
    let lines: Vec<Vec<RotatedRect>> = areas.iter().map(|&a| vec![crop(a)]).collect();
    let mut readings: Vec<Option<u16>> = engine
        .recognize_text(input, &lines)?
        .into_iter()
        .map(|text| as_number(&text?.to_string()))
        .collect();
    let mut recognized: Vec<Option<u16>> = vec![None; areas.len()];
    let mut corrections = 0;
    if compare_digits {
        for correction in templates::correct(&luma, &areas, &mut readings) {
            recognized[correction.number] = Some(correction.was);
            corrections += 1;
        }
    }
    // Each area's reading, in the same order as `areas`.
    let mut next = readings
        .into_iter()
        .zip(recognized)
        .zip(areas.iter().copied());
    let mut take = |lane: &Option<Vec<Area>>| -> Option<Vec<(Area, Option<u16>, Option<u16>)>> {
        lane.as_ref().map(|lane| {
            lane.iter()
                .map(|_| {
                    let ((number, recognized), area) = next.next().unwrap();
                    (area, number, recognized)
                })
                .collect()
        })
    };
    let col_runs: Vec<_> = cols.iter().map(&mut take).collect();
    let row_runs: Vec<_> = rows.iter().map(&mut take).collect();
    let col_slot_readings: Vec<_> = col_slotted.iter().map(&mut take).collect();
    let row_slot_readings: Vec<_> = row_slotted.iter().map(&mut take).collect();

    let mut found = vec![];
    let mut fallbacks = 0;
    let mut blots = 0;
    // The extra lane past the end needs more evidence than just reading as numbers (the edge of
    // a button can look like a "1"): they must be the size the others are, and in the middle of
    // the lane.
    let mut heights: Vec<i64> = cols[..width]
        .iter()
        .chain(&rows[..height])
        .flatten()
        .flatten()
        .map(|a| a.3 - a.1)
        .collect();
    heights.sort();
    let typical = heights.get(heights.len() / 2).copied().unwrap_or(0) as f32;
    let fits = |areas: &[Area], middle: f32, vertical: bool, pitch: f32| {
        areas.iter().all(|&(left, top, right, bottom)| {
            let tall = (bottom - top) as f32;
            let center = if vertical { left + right } else { top + bottom } as f32 / 2.0;
            (0.75..=1.25).contains(&(tall / typical)) && (center - middle).abs() < 0.2 * pitch
        })
    };
    let col_fits = |areas: &[Area]| fits(areas, col_center(width), true, layout.col_pitch);
    let row_fits = |areas: &[Area]| fits(areas, row_center(height), false, layout.row_pitch);

    type Readings = Option<Vec<(Area, Option<u16>, Option<u16>)>>;
    let mut read = |runs: Vec<Readings>,
                    slots: Vec<Readings>,
                    before: &[Vec<u16>],
                    plausible: &dyn Fn(&[Area]) -> bool|
     -> Vec<Vec<u16>> {
        let mut result = vec![];
        for (i, lane) in runs.into_iter().enumerate() {
            let all_read = |lane: &[(Area, Option<u16>, Option<u16>)]| {
                lane.iter().all(|(_, number, _)| number.is_some())
            };
            let Some(before) = before.get(i) else {
                // The extra lane past the end: keep it only if it's clearly clues.
                if let Some(lane) = lane {
                    let areas: Vec<Area> = lane.iter().map(|r| r.0).collect();
                    if !lane.is_empty() && all_read(&lane) && plausible(&areas) {
                        result.push(lane.iter().map(|r| r.1.unwrap()).collect());
                    }
                }
                break;
            };
            // As ink runs, if that read cleanly (but finding nothing where phase one found
            // something is more likely a miss); or by slot, if that did. Otherwise, what the first
            // pass said, if it said anything; or failing that, by slot, with whatever doesn't
            // read left blotted.
            let slot = slots.get(i).cloned().flatten().filter(|s| !s.is_empty());
            let chosen = match (lane, slot) {
                (Some(lane), _) if all_read(&lane) && !(lane.is_empty() && !before.is_empty()) => {
                    Some(lane)
                }
                (_, Some(slot)) if all_read(&slot) => Some(slot),
                _ if !before.is_empty() => None,
                (_, slot) => slot,
            };
            match chosen {
                Some(lane) => {
                    let mut numbers = vec![];
                    for (area, number, recognized) in lane {
                        found.push(Found {
                            area,
                            number,
                            recognized,
                        });
                        match number {
                            Some(0) => {}
                            Some(n) => numbers.push(n),
                            None => {
                                blots += 1;
                                numbers.push(BLOTTED);
                            }
                        }
                    }
                    result.push(numbers);
                }
                None => {
                    fallbacks += 1;
                    result.push(before.clone());
                }
            }
        }
        result
    };
    let new_cols = read(col_runs, col_slot_readings, &layout.cols, &col_fits);
    let new_rows = read(row_runs, row_slot_readings, &layout.rows, &row_fits);
    Ok(Reread {
        cols: new_cols,
        rows: new_rows,
        fallbacks,
        corrections,
        blots,
        found,
    })
}
