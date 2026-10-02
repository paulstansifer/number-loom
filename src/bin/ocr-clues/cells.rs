//! Reading the state of the grid: which cells the solver (the person) has filled in, crossed
//! out, or not decided yet.
//!
//! How an app draws each of those differs from app to app, but within one picture, every filled
//! cell looks like every other, and so on. So the cells are sorted into groups that look alike.
//! What each group *means* comes from the puzzle's answer, if the clues were read well enough to
//! solve it: a group of cells that are all filled in the answer is the player's filled cells; a
//! group that are all empty in the answer is their crossed-out ones; and a group that's a mix is
//! the cells they haven't decided yet. (So the groups' averages are templates for this app's way
//! of drawing cells, which could be used to read pictures whose clues can't be.)

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

/// The state of each cell, `[row][column]`, given whether it's filled in the answer (where that's
/// known).
pub fn read(
    image: &RgbImage,
    layout: &ClueLayout,
    answer: &[Vec<Option<bool>>],
) -> Vec<Vec<State>> {
    let (width, height) = (layout.col_centers.len(), layout.row_centers.len());
    // Stay clear of the grid lines.
    let half = 0.3 * layout.col_pitch.min(layout.row_pitch);
    let mut looks = vec![];
    for &y in &layout.row_centers {
        for &x in &layout.col_centers {
            looks.push(look(image, x, y, half));
        }
    }
    let mut states = vec![vec![State::Undecided; width]; height];
    if looks.len() < 8 {
        return states;
    }
    let assignment = groups(&looks, 6.min(looks.len() / 4));

    // Which state each group is: always filled in the answer, never, or both.
    let k = assignment.iter().max().unwrap() + 1;
    let mut filled = vec![0usize; k];
    let mut empty = vec![0usize; k];
    for (cell, &group) in assignment.iter().enumerate() {
        match answer[cell / width][cell % width] {
            Some(true) => filled[group] += 1,
            Some(false) => empty[group] += 1,
            None => {}
        }
    }
    let meaning: Vec<State> = (0..k)
        .map(|g| {
            let known = filled[g] + empty[g];
            if known < 3 {
                State::Undecided
            } else if filled[g] as f32 >= 0.95 * known as f32 {
                State::Filled
            } else if empty[g] as f32 >= 0.95 * known as f32 {
                State::Crossed
            } else {
                State::Undecided
            }
        })
        .collect();
    // Too few of a group's cells have a known answer to say what it is (a lone crossed-out cell
    // can be a group of its own): go by which group its cells look most like, if any is close.
    let centers: Vec<Vec<f32>> = (0..k)
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
    let spread: Vec<f32> = (0..k)
        .map(|g| {
            let mut distances: Vec<f32> = looks
                .iter()
                .zip(&assignment)
                .filter(|(_, a)| **a == g)
                .map(|(l, _)| distance(l, &centers[g]))
                .collect();
            distances.sort_by(f32::total_cmp);
            distances.get(distances.len() / 2).copied().unwrap_or(0.0)
        })
        .collect();
    let unlabeled = |g: usize| filled[g] + empty[g] < 3;
    for (cell, &group) in assignment.iter().enumerate() {
        let mut state = meaning[group];
        if unlabeled(group) {
            let nearest = (0..k)
                .filter(|&g| !unlabeled(g) && meaning[g] != State::Undecided)
                .min_by(|&a, &b| {
                    distance(&looks[cell], &centers[a])
                        .total_cmp(&distance(&looks[cell], &centers[b]))
                });
            if let Some(g) = nearest
                && distance(&looks[cell], &centers[g]) <= 3.0 * spread[g]
            {
                state = meaning[g];
            }
        }
        states[cell / width][cell % width] = state;
    }
    states
}
