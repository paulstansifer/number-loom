//! Phase three: every clue in a picture is in the same typeface, at the same size, so each digit
//! should look like the others that are the same digit. Recognition is right most of the time,
//! so the average of all the digits it read as, say, "1" is a good picture of a "1" in this
//! typeface. A digit that looks much more like that than like the other digits recognition said
//! were the same as it was probably misread.
//!
//! Digits are compared as "inkiness" (how far from the background, relative to the digit's own
//! contrast, so grayed-out clues compare fine), scaled to a common height but keeping their
//! shape, since a "1" is narrower than a "7".

use crate::reread::{Area, Luma};

const WIDTH: usize = 20;
const HEIGHT: usize = 24;

/// One digit of one of the numbers, as `WIDTH * HEIGHT` inkiness values.
struct Sample {
    /// Which number (index into the readings) and which digit of it.
    number: usize,
    place: usize,
    digit: u8,
    pixels: Vec<f32>,
}

/// How inky each pixel of `area` is, from 0 (background) to 1, as a function.
fn inkiness(luma: &Luma, area: Area) -> Option<impl Fn(i64, i64) -> f32 + '_> {
    let (left, top, right, bottom) = area;
    // The background: what's just outside the area.
    let margin = 3;
    let mut ring: Vec<f32> = (top - margin..=bottom + margin)
        .flat_map(|y| (left - margin..=right + margin).map(move |x| (x, y)))
        .filter(|&(x, y)| x < left || x > right || y < top || y > bottom)
        .filter_map(|(x, y)| luma.at(x, y))
        .collect();
    ring.sort_by(f32::total_cmp);
    let background = *ring.get(ring.len() / 2)?;
    // The ink's own contrast: a tight box around a digit is a good part ink, so take a high
    // percentile (not the maximum, which might be some stray mark).
    let mut contrast: Vec<f32> = (top..=bottom)
        .flat_map(|y| (left..=right).filter_map(move |x| luma.at(x, y)))
        .map(|v| (v - background).abs())
        .collect();
    contrast.sort_by(f32::total_cmp);
    let contrast = *contrast.get(contrast.len() * 9 / 10)?;
    (contrast > 0.0).then(move || {
        move |x, y| {
            luma.at(x, y)
                .map_or(0.0, |v| ((v - background).abs() / contrast).min(1.0))
        }
    })
}

/// Split a number's area into its digits, by the gaps in the ink.
fn split(ink: &impl Fn(i64, i64) -> f32, (left, top, right, bottom): Area) -> Vec<Area> {
    let inky = |x: i64, y: i64| ink(x, y) > 0.4;
    let mut digits: Vec<Area> = vec![];
    let mut start = None;
    for x in left..=right + 1 {
        let on = x <= right && (top..=bottom).any(|y| inky(x, y));
        match (on, start) {
            (true, None) => start = Some(x),
            (false, Some(s)) => {
                start = None;
                // (A one-pixel break is antialiasing, not a gap.)
                if let Some(last) = digits.last_mut()
                    && s - last.2 <= 2
                {
                    last.2 = x - 1;
                    continue;
                }
                digits.push((s, top, x - 1, bottom));
            }
            _ => {}
        }
    }
    // Trim each to its own ink, top and bottom.
    for digit in &mut digits {
        let rows: Vec<i64> = (top..=bottom)
            .filter(|&y| (digit.0..=digit.2).any(|x| inky(x, y)))
            .collect();
        if let (Some(&first), Some(&last)) = (rows.first(), rows.last()) {
            (digit.1, digit.3) = (first, last);
        }
    }
    digits
}

/// The digit in `area`, scaled to `HEIGHT` and centered in `WIDTH`.
fn normalize(ink: &impl Fn(i64, i64) -> f32, (left, top, right, bottom): Area) -> Vec<f32> {
    let (w, h) = ((right - left + 1) as f32, (bottom - top + 1) as f32);
    let scale = (HEIGHT as f32 / h).min(WIDTH as f32 / w);
    let offset_x = (WIDTH as f32 - w * scale) / 2.0;
    let offset_y = (HEIGHT as f32 - h * scale) / 2.0;
    let mut pixels = vec![0.0; WIDTH * HEIGHT];
    for cy in 0..HEIGHT {
        for cx in 0..WIDTH {
            // Bilinear sampling, of the pixel center.
            let sx = left as f32 + (cx as f32 + 0.5 - offset_x) / scale - 0.5;
            let sy = top as f32 + (cy as f32 + 0.5 - offset_y) / scale - 0.5;
            if sx < left as f32 - 0.5 || sx > right as f32 + 0.5 {
                continue;
            }
            if sy < top as f32 - 0.5 || sy > bottom as f32 + 0.5 {
                continue;
            }
            let (x0, y0) = (sx.floor(), sy.floor());
            let (fx, fy) = (sx - x0, sy - y0);
            let at = |dx: i64, dy: i64| ink(x0 as i64 + dx, y0 as i64 + dy).min(1.0);
            pixels[cy * WIDTH + cx] = at(0, 0) * (1.0 - fx) * (1.0 - fy)
                + at(1, 0) * fx * (1.0 - fy)
                + at(0, 1) * (1.0 - fx) * fy
                + at(1, 1) * fx * fy;
        }
    }
    pixels
}

fn distance(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(a, b)| (a - b).powi(2)).sum()
}

fn digits_of(n: u16) -> Vec<u8> {
    if n >= 10 {
        vec![(n / 10) as u8, (n % 10) as u8]
    } else {
        vec![n as u8]
    }
}

/// Relabel the samples that look more like a different digit. Returns whether any changed.
fn relabel(samples: &mut [Sample]) -> bool {
    let mut sums = vec![vec![0.0; WIDTH * HEIGHT]; 10];
    let mut counts = [0usize; 10];
    for s in samples.iter() {
        counts[s.digit as usize] += 1;
        for (sum, p) in sums[s.digit as usize].iter_mut().zip(&s.pixels) {
            *sum += p;
        }
    }
    // The average of digit `d`, leaving `sample` out of it (if it's one of them). `None` if
    // there aren't at least two others to go on.
    let template = |d: usize, sample: &Sample| -> Option<Vec<f32>> {
        let own = sample.digit as usize == d;
        let n = counts[d] - usize::from(own);
        (n >= 2).then(|| {
            sums[d]
                .iter()
                .zip(&sample.pixels)
                .map(|(sum, p)| (sum - if own { *p } else { 0.0 }) / n as f32)
                .collect()
        })
    };
    // How far from the average a digit typically is.
    let spread: Vec<Option<f32>> = (0..10)
        .map(|d| {
            let mut distances: Vec<f32> = samples
                .iter()
                .filter(|s| s.digit as usize == d)
                .filter_map(|s| Some(distance(&s.pixels, &template(d, s)?)))
                .collect();
            distances.sort_by(f32::total_cmp);
            distances.get(distances.len() / 2).copied()
        })
        .collect();

    let mut changes = vec![];
    for (i, s) in samples.iter().enumerate() {
        let own = template(s.digit as usize, s).map(|t| distance(&s.pixels, &t));
        let Some((best, d)) = (0..10)
            .filter(|&d| d != s.digit as usize)
            .filter_map(|d| Some((d, distance(&s.pixels, &template(d, s)?))))
            .min_by(|a, b| a.1.total_cmp(&b.1))
        else {
            continue;
        };
        // It has to look much more like a `best` than like its own kind, or if there aren't
        // enough of its own kind to say, like a typical `best`.
        let better = match own {
            Some(own) => d < 0.7 * own,
            None => spread[best].is_some_and(|spread| d <= 1.5 * spread),
        };
        if better {
            changes.push((i, best as u8));
        }
    }
    for &(i, digit) in &changes {
        samples[i].digit = digit;
    }
    !changes.is_empty()
}

/// A digit that was read as one thing, but looked like another.
pub struct Correction {
    pub number: usize,
    pub was: u16,
}

/// Fix `readings` (of the numbers in `areas`) where a digit looks like a different digit.
pub fn correct(luma: &Luma, areas: &[Area], readings: &mut [Option<u16>]) -> Vec<Correction> {
    let mut samples = vec![];
    for (number, (&area, reading)) in areas.iter().zip(readings.iter()).enumerate() {
        let (Some(n), Some(ink)) = (reading, inkiness(luma, area)) else {
            continue;
        };
        let digits = digits_of(*n);
        let boxes = split(&ink, area);
        // If the ink doesn't split into as many pieces as there are digits, it can't say which
        // is which.
        if boxes.len() != digits.len() {
            continue;
        }
        for (place, (digit, area)) in digits.into_iter().zip(boxes).enumerate() {
            samples.push(Sample {
                number,
                place,
                digit,
                pixels: normalize(&ink, area),
            });
        }
    }

    // Relabel, then compare against the improved averages, a few times over: a misread digit
    // blurs the average of what it was misread as.
    let original: Vec<u8> = samples.iter().map(|s| s.digit).collect();
    for _ in 0..3 {
        if !relabel(&mut samples) {
            break;
        }
    }
    let relabeled: Vec<(usize, usize, u8)> = samples
        .iter()
        .zip(original)
        .filter(|(s, was)| s.digit != *was)
        .map(|(s, _)| (s.number, s.place, s.digit))
        .collect();

    let mut corrections = vec![];
    for (number, place, digit) in relabeled {
        let Some(was) = readings[number] else {
            continue;
        };
        let mut digits = digits_of(was);
        digits[place] = digit;
        let now = digits.iter().fold(0, |n, &d| n * 10 + d as u16);
        // (Two corrections to one number: the first's already in `readings`.)
        let original = corrections
            .iter()
            .find(|c: &&Correction| c.number == number)
            .map_or(was, |c| c.was);
        corrections.retain(|c| c.number != number);
        readings[number] = Some(now);
        corrections.push(Correction {
            number,
            was: original,
        });
    }
    corrections
}
