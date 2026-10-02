//! Phase two: once `clue_layout` has found the grid, look for each lane's clues right where they
//! must be, instead of trusting the first pass's detection.
//!
//! The pixels say where each number is: a column's clues are lines of ink stacked above the
//! column, and a row's clues are runs of ink left of the row, split into numbers where the gaps
//! are wide. (Measured on the ink itself, the space between two numbers is reliably wider than
//! the space between the digits of one, which OCR's character boxes are too rough to show.) Each
//! number is then recognized as a crop of its own.

use crate::clue_layout::{ClueLayout, split_threshold};
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

/// The clues of the column centered at `center`, from the top.
fn column(luma: &Luma, layout: &ClueLayout, center: f32) -> Lane {
    let h = layout.glyph_height;
    let pitch = layout.col_pitch;
    let (left, right) = (
        (center - 0.45 * pitch) as i64,
        (center + 0.45 * pitch) as i64,
    );
    let bottom = (layout.grid_top.at(center) + 0.25 * h) as i64;
    let top = (bottom as f32 - 12.0 * h).max(0.0) as i64;
    // Steps go up from the grid.
    let y = |i: usize| bottom - i as i64;
    let steps = (bottom - top + 1) as usize;
    let strip = Strip::new(
        steps,
        |i| (left..=right).filter_map(|x| luma.at(x, y(i))).collect(),
        (1.2 * h) as usize,
    );
    let is_ink = |x: i64, i: usize| strip.is_ink(i, luma.at(x, y(i)));

    // A step has ink if any pixel across it does, unless it's a rule across the whole column.
    let width = (right - left + 1) as f32;
    let profile: Vec<bool> = (0..steps)
        .map(|i| {
            let count = (left..=right).filter(|&x| is_ink(x, i)).count() as f32;
            count >= 1.0 && count < 0.85 * width
        })
        .collect();
    let mut areas = vec![];
    let mut last_end = 0;
    for (start, end) in runs(&profile, 0.1 * h) {
        // A big gap is the end of the clues (or there were none).
        if start as f32 - last_end as f32 > 1.5 * h {
            break;
        }
        let tall = (end - start + 1) as f32;
        if tall < 0.3 * h {
            continue; // a speck
        }
        if tall > 1.6 * h {
            return None;
        }
        // Across the line, the ink nearest the middle, plus anything close enough to be another
        // digit of the same number. (Not ink at the edges, from a neighbor's two-digit number.)
        let across: Vec<bool> = (left..=right)
            .map(|x| (start..=end).any(|i| is_ink(x, i)))
            .collect();
        let mid = (center - left as f32) as usize;
        let mut pieces = runs(&across, 0.4 * h);
        pieces.sort_by_key(|&(a, b)| {
            if a <= mid && mid <= b {
                0
            } else {
                a.abs_diff(mid).min(b.abs_diff(mid))
            }
        });
        let (x0, x1) = *pieces.first()?;
        areas.push((left + x0 as i64, y(end), left + x1 as i64, y(start)));
        last_end = end;
    }
    areas.reverse();
    Some(areas)
}

/// The clues of the row centered at `center`, as the areas of the individual digits (or other
/// marks), from the left.
fn row_marks(luma: &Luma, layout: &ClueLayout, center: f32) -> Lane {
    let h = layout.glyph_height;
    let pitch = layout.row_pitch;
    let (top, bottom) = (
        (center - 0.42 * pitch) as i64,
        (center + 0.42 * pitch) as i64,
    );
    let right = (layout.grid_left.at(center) + 0.25 * h) as i64;
    let left = (right as f32 - 25.0 * h).max(0.0) as i64;
    // Steps go left from the grid.
    let x = |i: usize| right - i as i64;
    let steps = (right - left + 1) as usize;
    let strip = Strip::new(
        steps,
        |i| (top..=bottom).filter_map(|y| luma.at(x(i), y)).collect(),
        (1.2 * h) as usize,
    );
    let is_ink = |i: usize, y: i64| strip.is_ink(i, luma.at(x(i), y));

    let height = (bottom - top + 1) as f32;
    let profile: Vec<bool> = (0..steps)
        .map(|i| {
            let count = (top..=bottom).filter(|&y| is_ink(i, y)).count() as f32;
            count >= 1.0 && count < 0.85 * height
        })
        .collect();
    let mut marks = vec![];
    let mut last_end = 0;
    for (start, end) in runs(&profile, 0.1 * h) {
        if marks.is_empty() && start as f32 > 1.6 * h {
            return Some(vec![]); // nothing next to the grid
        }
        if start as f32 - last_end as f32 > 1.6 * h {
            break;
        }
        if (end - start + 1) as f32 > 1.5 * h {
            return None; // too wide to be a digit
        }
        let ys: Vec<i64> = (top..=bottom)
            .filter(|&y| (start..=end).any(|i| is_ink(i, y)))
            .collect();
        let (y0, y1) = (*ys.first()?, *ys.last()?);
        if ((y1 - y0 + 1) as f32) < 0.3 * h {
            continue; // a speck, or a strikethrough's stray end
        }
        marks.push((x(end), y0, x(start), y1));
        last_end = end;
    }
    marks.reverse();
    Some(marks)
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
) -> anyhow::Result<Reread> {
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

    // Group a row's marks into numbers, by the gaps between them. All the rows share a
    // typeface, so decide what's a wide gap from all of them together.
    let gaps: Vec<f32> = row_marks
        .iter()
        .flatten()
        .flat_map(|marks| marks.windows(2).map(|w| (w[1].0 - w[0].2 - 1) as f32))
        .collect();
    let threshold = split_threshold(gaps, 0.45 * h, 1.5);
    let rows: Vec<Lane> = row_marks
        .into_iter()
        .map(|marks| {
            let mut numbers: Vec<Area> = vec![];
            for mark in marks? {
                match numbers.last_mut() {
                    Some(last) if ((mark.0 - last.2 - 1) as f32) < threshold => {
                        *last = union(*last, mark)
                    }
                    _ => numbers.push(mark),
                }
            }
            Some(numbers)
        })
        .collect();

    // Recognize every number at once.
    let areas: Vec<Area> = cols
        .iter()
        .chain(&rows)
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
    let mut readings = readings.into_iter().zip(recognized);
    let mut found = vec![];
    let mut fallbacks = 0;
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

    let mut read = |lanes: Vec<Lane>,
                    before: &[Vec<u16>],
                    plausible: &dyn Fn(&[Area]) -> bool|
     -> Vec<Vec<u16>> {
        let mut result = vec![];
        for (i, lane) in lanes.into_iter().enumerate() {
            let plausible = lane.as_deref().is_some_and(plausible);
            // Each number, or `None` if it wasn't one.
            let numbers: Option<Vec<Option<u16>>> = lane.map(|areas| {
                areas
                    .into_iter()
                    .map(|area| {
                        let (number, recognized) = readings.next().unwrap_or_default();
                        found.push(Found {
                            area,
                            number,
                            recognized,
                        });
                        number
                    })
                    .collect()
            });
            let numbers: Option<Vec<u16>> = numbers.and_then(|n| n.into_iter().collect());
            let Some(before) = before.get(i) else {
                // The extra lane past the end: keep it only if it's clearly clues.
                if let Some(numbers) = numbers.filter(|n| !n.is_empty() && plausible) {
                    result.push(numbers);
                }
                break;
            };
            match numbers {
                // Finding nothing where phase one found something is more likely a miss.
                Some(numbers) if !(numbers.is_empty() && !before.is_empty()) => {
                    result.push(numbers.into_iter().filter(|&n| n != 0).collect())
                }
                _ => {
                    fallbacks += 1;
                    result.push(before.clone());
                }
            }
        }
        result
    };
    let new_cols = read(cols, &layout.cols, &col_fits);
    let new_rows = read(rows, &layout.rows, &row_fits);
    Ok(Reread {
        cols: new_cols,
        rows: new_rows,
        fallbacks,
        corrections,
        found,
    })
}
