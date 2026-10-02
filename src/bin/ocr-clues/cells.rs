//! Reading the state of the grid: which cells the solver (the person) has filled in, crossed
//! out, or not decided yet.
//!
//! How an app draws each of those differs from app to app, but within one picture, every filled
//! cell looks like every other, and so on. So the cells are sorted into groups that look alike,
//! and what each group means comes from how it looks: a group with something drawn in it (an X,
//! or a dot) is crossed-out cells, and a plain group that's clearly darker than the rest is
//! filled cells. This doesn't need the clues, which matters because the more of a puzzle is
//! solved, the more of its clues are likely to be crossed out.

use image::RgbImage;

use crate::clue_layout::ClueLayout;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Filled,
    Crossed,
    Undecided,
}

/// Each cell's look: its middle, shrunk to `SIZE` by `SIZE`, in RGB.
const SIZE: usize = 8;

fn look(image: &RgbImage, x: f32, y: f32, half: f32) -> Vec<f32> {
    let mut features = vec![0.0; SIZE * SIZE * 3];
    let mut counts = vec![0.0; SIZE * SIZE];
    let (x0, y0) = (x - half, y - half);
    let side = 2.0 * half;
    for py in (y0 as i64)..=((y0 + side) as i64) {
        for px in (x0 as i64)..=((x0 + side) as i64) {
            if px < 0 || py < 0 || px >= image.width() as i64 || py >= image.height() as i64 {
                continue;
            }
            let bx = (((px as f32 - x0) / side * SIZE as f32) as usize).min(SIZE - 1);
            let by = (((py as f32 - y0) / side * SIZE as f32) as usize).min(SIZE - 1);
            let pixel = image.get_pixel(px as u32, py as u32);
            for channel in 0..3 {
                features[(by * SIZE + bx) * 3 + channel] += pixel[channel] as f32 / 255.0;
            }
            counts[by * SIZE + bx] += 1.0;
        }
    }
    for (i, f) in features.iter_mut().enumerate() {
        *f /= f32::max(counts[i / 3], 1.0);
    }
    features
}

fn distance(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(a, b)| (a - b).powi(2)).sum()
}

/// k-means, started from points spread far apart. Returns each point's group.
fn groups(points: &[Vec<f32>], k: usize) -> Vec<usize> {
    let dims = points[0].len();
    let mean: Vec<f32> = (0..dims)
        .map(|d| points.iter().map(|p| p[d]).sum::<f32>() / points.len() as f32)
        .collect();
    let farthest_from = |centers: &[Vec<f32>]| -> Vec<f32> {
        points
            .iter()
            .max_by(|a, b| {
                let near = |p: &Vec<f32>| {
                    centers
                        .iter()
                        .map(|c| distance(p, c))
                        .fold(f32::INFINITY, f32::min)
                };
                near(a).total_cmp(&near(b))
            })
            .unwrap()
            .clone()
    };
    let mut centers = vec![farthest_from(&[mean])];
    while centers.len() < k {
        let next = farthest_from(&centers);
        centers.push(next);
    }

    let mut assignment = vec![0; points.len()];
    for _ in 0..20 {
        for (p, a) in points.iter().zip(&mut assignment) {
            *a = (0..k)
                .min_by(|&i, &j| distance(p, &centers[i]).total_cmp(&distance(p, &centers[j])))
                .unwrap();
        }
        for (i, center) in centers.iter_mut().enumerate() {
            let members: Vec<&Vec<f32>> = points
                .iter()
                .zip(&assignment)
                .filter(|(_, a)| **a == i)
                .map(|(p, _)| p)
                .collect();
            if !members.is_empty() {
                for d in 0..dims {
                    center[d] = members.iter().map(|p| p[d]).sum::<f32>() / members.len() as f32;
                }
            }
        }
    }
    assignment
}

/// Each cell's look, `[row * width + column]`, and which group of look-alikes it's in.
fn grouped(image: &RgbImage, layout: &ClueLayout) -> Option<(Vec<Vec<f32>>, Vec<usize>)> {
    // Stay clear of the grid lines.
    let half = 0.3 * layout.col_pitch.min(layout.row_pitch);
    let mut looks = vec![];
    for &y in &layout.row_centers {
        for &x in &layout.col_centers {
            looks.push(look(image, x, y, half));
        }
    }
    if looks.len() < 8 {
        return None;
    }
    let assignment = groups(&looks, 6.min(looks.len() / 4));
    Some((looks, assignment))
}

/// Brightness, from 0 to 1, of each pixel of a look.
fn brightness(look: &[f32]) -> Vec<f32> {
    look.chunks(3)
        .map(|p| 0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2])
        .collect()
}

/// The state of each cell, `[row][column]`, from how the groups of
/// look-alike cells look. A group with something drawn in it (an X, or a dot) is crossed-out
/// cells, and the color behind the mark is what an undecided cell looks like. A plain group
/// that's clearly darker than that is filled cells. (Darker is filled in every black-and-white
/// puzzle we've seen, dark-mode apps included; a colored puzzle on a dark background can break
/// that.)
pub fn read(image: &RgbImage, layout: &ClueLayout) -> Vec<Vec<State>> {
    let (width, height) = (layout.col_centers.len(), layout.row_centers.len());
    let mut states = vec![vec![State::Undecided; width]; height];
    let Some((looks, assignment)) = grouped(image, layout) else {
        return states;
    };
    let k = assignment.iter().max().unwrap() + 1;
    // Each group's average look, its brightness, and how much variety there is within it.
    let average: Vec<Vec<f32>> = (0..k)
        .map(|g| {
            let members: Vec<&Vec<f32>> = looks
                .iter()
                .zip(&assignment)
                .filter(|(_, a)| **a == g)
                .map(|(l, _)| l)
                .collect();
            (0..looks[0].len())
                .map(|d| members.iter().map(|m| m[d]).sum::<f32>() / members.len().max(1) as f32)
                .collect()
        })
        .collect();
    // Each group's typical brightness, and how much it varies within a cell (a mark drawn in it
    // makes it vary a lot; a plain cell, filled or not, hardly at all).
    let looks_of: Vec<(f32, f32)> = average
        .iter()
        .map(|a| {
            let b = brightness(a);
            let mean = b.iter().sum::<f32>() / b.len() as f32;
            let spread =
                (b.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / b.len() as f32).sqrt();
            let mut sorted = b;
            sorted.sort_by(f32::total_cmp);
            (sorted[sorted.len() / 2], spread)
        })
        .collect();
    let marked = |g: usize| looks_of[g].1 > 0.06;
    let mut meaning = vec![State::Undecided; k];
    for g in (0..k).filter(|&g| marked(g)) {
        meaning[g] = State::Crossed;
    }
    // Behind the marks is what an undecided cell looks like.
    let behind_marks: Vec<f32> = (0..k)
        .filter(|&g| marked(g))
        .map(|g| looks_of[g].0)
        .collect();
    let undecided_brightness = (!behind_marks.is_empty())
        .then(|| behind_marks.iter().sum::<f32>() / behind_marks.len() as f32);
    // The plain groups split at the biggest gap in brightness, if there's a clear one: darker
    // is filled. (Uneven lighting can make undecided cells several groups of their own.)
    let mut plain: Vec<usize> = (0..k).filter(|&g| !marked(g)).collect();
    plain.sort_by(|&a, &b| looks_of[a].0.total_cmp(&looks_of[b].0));
    let darker_than_undecided =
        |g: usize| undecided_brightness.is_none_or(|u| looks_of[g].0 < u - 0.2);
    if plain.len() >= 2 {
        let (gap, at) = plain
            .windows(2)
            .enumerate()
            .map(|(i, w)| (looks_of[w[1]].0 - looks_of[w[0]].0, i + 1))
            .max_by(|a, b| a.0.total_cmp(&b.0))
            .unwrap();
        if gap >= 0.15 {
            for &g in &plain[..at] {
                if darker_than_undecided(g) {
                    meaning[g] = State::Filled;
                }
            }
        }
    } else if let [g] = plain[..]
        && undecided_brightness.is_some()
        && darker_than_undecided(g)
    {
        // Only one kind of plain cell, and it's clearly not what's behind the marks.
        meaning[g] = State::Filled;
    }
    for (cell, &group) in assignment.iter().enumerate() {
        states[cell / width][cell % width] = meaning[group];
    }
    states
}
