//! Arranging digits found in a picture of a puzzle into row and column clues.
//!
//! The input is a pile of positioned digits (from OCR), some of which are noise. Every clue digit
//! sits on a vertical *chain* (digits stacked one above the other) and on a horizontal one, so
//! being aligned doesn't say whether a digit belongs to a row clue or a column clue. Where chains
//! *end* does: column clues are bottom-aligned against the grid, so neighboring columns' vertical
//! chains all end on the same line (the grid's top edge), and row clues are right-aligned, so
//! neighboring rows' horizontal chains all end on the grid's left edge. Chains of the wrong kind
//! end all over the place.
//!
//! Once those two edges are found, they split the picture into a column-clue region and a
//! row-clue region. The spacing of the clues within each gives the size of a cell, so each digit
//! has a slot (which column, or which row) by rounding. Only then are digits joined into numbers:
//! uniformly spaced digits can't otherwise be told apart from a two-digit number (in a column
//! clue, "1010" sits across two columns).

use std::collections::BTreeMap;

/// One digit, as found by OCR. Coordinates are in image pixels, with y pointing down.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glyph {
    pub digit: u8,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Glyph {
    fn bottom(&self) -> f32 {
        self.y + self.height / 2.0
    }
    fn right(&self) -> f32 {
        self.x + self.width / 2.0
    }
}

/// What a glyph turned out to be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Ignored,
    Row(usize),
    Col(usize),
}

/// A line `along = intercept + slope * across`. The grid's top edge is `y` as a function of
/// `x`; its left edge, `x` as a function of `y`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Edge {
    pub intercept: f32,
    pub slope: f32,
}

impl Edge {
    pub fn at(&self, across: f32) -> f32 {
        self.intercept + self.slope * across
    }

    /// Least-squares fit through `(across, along)` points. If `typical_height` is nonzero, this is
    /// an edge of the grid, and only a gentle tilt is believable.
    fn fit(points: &[(f32, f32)], typical_height: f32) -> Edge {
        let n = points.len() as f32;
        let mean_across = points.iter().map(|p| p.0).sum::<f32>() / n;
        let mean_along = points.iter().map(|p| p.1).sum::<f32>() / n;
        let spread: f32 = points.iter().map(|p| (p.0 - mean_across).powi(2)).sum();
        let covariance: f32 = points
            .iter()
            .map(|p| (p.0 - mean_across) * (p.1 - mean_along))
            .sum();
        // Too short a run of points to trust for a tilt, or a tilt no photo would have.
        let mut slope = if spread > (3.0 * typical_height).powi(2) * n {
            covariance / spread
        } else {
            0.0
        };
        if typical_height > 0.0 && slope.abs() > 0.1 {
            slope = 0.0;
        }
        Edge {
            intercept: mean_along - slope * mean_across,
            slope,
        }
    }
}

/// A clue that's there, but couldn't be read (say, because it's been crossed out).
pub const BLOTTED: u16 = u16::MAX;

/// A clue as text: the number, or "?" if it's blotted.
pub fn clue_text(n: u16) -> String {
    if n == BLOTTED {
        "?".to_string()
    } else {
        n.to_string()
    }
}

#[derive(Clone, Debug)]
pub struct ClueLayout {
    pub rows: Vec<Vec<u16>>,
    pub cols: Vec<Vec<u16>>,
    /// Parallel to the input glyphs.
    pub roles: Vec<Role>,
    pub grid_top: Edge,
    pub grid_left: Edge,
    /// Where the middle of each column is (x), and each row (y).
    pub col_centers: Vec<f32>,
    pub row_centers: Vec<f32>,
    /// The size of a cell.
    pub col_pitch: f32,
    pub row_pitch: f32,
    /// How tall a digit is.
    pub glyph_height: f32,
    /// Whether `grid_top` and `grid_left` are the grid's border lines (found from its lines), as
    /// opposed to where its clues end.
    pub edges_on_lines: bool,
    /// Things that look wrong, for a human to check.
    pub warnings: Vec<String>,
}

fn median(mut v: Vec<f32>) -> Option<f32> {
    v.sort_by(f32::total_cmp);
    v.get(v.len() / 2).copied()
}

/// A tiny union-find.
struct Sets(Vec<usize>);

impl Sets {
    fn new(n: usize) -> Sets {
        Sets((0..n).collect())
    }
    fn find(&mut self, i: usize) -> usize {
        if self.0[i] != i {
            let root = self.find(self.0[i]);
            self.0[i] = root;
        }
        self.0[i]
    }
    fn join(&mut self, a: usize, b: usize) {
        let (a, b) = (self.find(a), self.find(b));
        self.0[a] = b;
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Axis {
    /// Clues stacked top-to-bottom (column clues).
    Vertical,
    /// Clues side-by-side (row clues).
    Horizontal,
}

impl Axis {
    /// `(along, across)`: position along the chain, and position across it.
    fn coords(self, g: &Glyph) -> (f32, f32) {
        match self {
            Axis::Vertical => (g.y, g.x),
            Axis::Horizontal => (g.x, g.y),
        }
    }

    /// Where a chain that ends with `g` stops: the far edge of `g`.
    fn end(self, g: &Glyph) -> (f32, f32) {
        match self {
            Axis::Vertical => (g.bottom(), g.x),
            Axis::Horizontal => (g.right(), g.y),
        }
    }

    /// How far apart neighbors in a chain may be, and how far out of line, in glyph heights.
    /// Column clues are stacked at about the line spacing; row clues can be spread as far apart
    /// as the cells are wide.
    fn reach(self) -> (f32, f32) {
        match self {
            Axis::Vertical => (2.2, 0.6),
            Axis::Horizontal => (3.0, 0.4),
        }
    }
}

/// For each glyph, its successor in a chain along `axis`, if any. Links are only kept when each
/// glyph is the other's nearest neighbor (in the right direction), so chains don't branch.
fn chain_links(glyphs: &[Glyph], axis: Axis, h: f32) -> Vec<Option<usize>> {
    let (reach, skew) = axis.reach();
    let nearest = |i: usize, forward: bool| -> Option<usize> {
        let (along_i, across_i) = axis.coords(&glyphs[i]);
        (0..glyphs.len())
            .filter_map(|j| {
                let (along_j, across_j) = axis.coords(&glyphs[j]);
                let d = if forward {
                    along_j - along_i
                } else {
                    along_i - along_j
                };
                let off = (across_j - across_i).abs();
                (d > 0.2 * h && d <= reach * h && off < skew * h).then_some((j, d + off))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(j, _)| j)
    };
    (0..glyphs.len())
        .map(|i| nearest(i, true).filter(|&j| nearest(j, false) == Some(i)))
        .collect()
}

/// The clues for one direction, as found by the chain ends.
struct Block {
    axis: Axis,
    /// Glyphs that end a chain on the shared edge.
    ends: Vec<usize>,
    edge: Edge,
    links: Vec<Option<usize>>,
}

impl Block {
    /// The glyphs in this block: those on chains that run up to the edge without leaving the
    /// region (stray marks in the grid can extend a chain past it), plus any digits close
    /// enough to those to have been left out of a chain by mistake (the other digit of a
    /// two-digit number, or a digit below one OCR missed).
    fn members(&self, glyphs: &[Glyph], inside: impl Fn(&Glyph) -> bool, h: f32) -> Vec<bool> {
        let (reach, _) = self.axis.reach();
        let mut member = vec![false; glyphs.len()];
        let mut todo = vec![];
        let acrosses = self.ends.iter().map(|&i| self.axis.end(&glyphs[i]).1);
        let (first, last) = (
            acrosses.clone().fold(f32::INFINITY, f32::min),
            acrosses.fold(f32::NEG_INFINITY, f32::max),
        );
        let (inner, outer) = (first..=last, first - 3.0 * h..=last + 3.0 * h);
        for i in 0..glyphs.len() {
            if !inside(&glyphs[i]) {
                continue;
            }
            let (mut end, mut len) = (i, 1);
            while let Some(next) = self.links[end].filter(|&n| inside(&glyphs[n])) {
                (end, len) = (next, len + 1);
            }
            // On the edge, or, within the block, short of it by about one missing digit. Just
            // past either end of the block, a lone digit is more likely a stray mark.
            let (along, across) = self.axis.end(&glyphs[end]);
            let short = (along - self.edge.at(across)).abs();
            let within = inner.contains(&across) || (len >= 2 && outer.contains(&across));
            if short < 0.5 * h || (short < (reach + 0.5) * h && within) {
                member[i] = true;
                todo.push(i);
            }
        }
        // Hops: to a neighbor in the same chain, or across a gap of about one missing digit.
        let (hop_along, hop_across) = match self.axis {
            Axis::Vertical => (2.2 * h, 0.8 * h),
            Axis::Horizontal => (reach * h, 0.5 * h),
        };
        while let Some(i) = todo.pop() {
            for j in 0..glyphs.len() {
                if member[j] || !inside(&glyphs[j]) {
                    continue;
                }
                let (along_i, across_i) = self.axis.coords(&glyphs[i]);
                let (along_j, across_j) = self.axis.coords(&glyphs[j]);
                if (along_i - along_j).abs() < hop_along && (across_i - across_j).abs() < hop_across
                {
                    member[j] = true;
                    todo.push(j);
                }
            }
        }
        member
    }
}

/// Find the biggest group of chains along `axis` that all stop at the same place, with each
/// chain's end close to the next one's.
fn find_block(glyphs: &[Glyph], axis: Axis, h: f32) -> Option<Block> {
    let links = chain_links(glyphs, axis, h);
    let ends: Vec<usize> = (0..glyphs.len()).filter(|&i| links[i].is_none()).collect();

    let mut sets = Sets::new(ends.len());
    for a in 0..ends.len() {
        for b in a + 1..ends.len() {
            let (along_a, across_a) = axis.end(&glyphs[ends[a]]);
            let (along_b, across_b) = axis.end(&glyphs[ends[b]]);
            // Lined up, and not too far apart. (A gap of a few empty lines is fine.)
            if (along_a - along_b).abs() < 0.5 * h && (across_a - across_b).abs() < 6.0 * h {
                sets.join(a, b);
            }
        }
    }
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (k, &i) in ends.iter().enumerate() {
        groups.entry(sets.find(k)).or_default().push(i);
    }
    let ends = groups.into_values().max_by_key(|g| g.len())?;
    if ends.len() < 2 {
        return None;
    }
    let points: Vec<(f32, f32)> = ends
        .iter()
        .map(|&i| {
            let (along, across) = axis.end(&glyphs[i]);
            (across, along)
        })
        .collect();
    let edge = Edge::fit(&points, h);
    Some(Block {
        axis,
        ends,
        edge,
        links,
    })
}

/// The lattice `phase + n * pitch` that `positions` (with gaps, and near-duplicates) sit on, as
/// `(pitch, phase)`.
fn lattice(positions: &[f32], h: f32) -> Option<(f32, f32)> {
    let mut sorted = positions.to_vec();
    sorted.sort_by(f32::total_cmp);
    // Positions in one line (a row's digits, or the two digits of a number in a column) are
    // nearly the same. Average them, or the distance from the last of one line to the first of
    // the next would be a little short, and that adds up.
    let mut lines: Vec<Vec<f32>> = vec![];
    for &p in &sorted {
        match lines.last_mut() {
            Some(line) if p - line.last().unwrap() < 0.75 * h => line.push(p),
            _ => lines.push(vec![p]),
        }
    }
    // (A median, so a speck beside a column doesn't pull it off-center.)
    let sorted: Vec<f32> = lines
        .iter()
        .map(|l| (l[(l.len() - 1) / 2] + l[l.len() / 2]) / 2.0)
        .collect();
    let diffs: Vec<f32> = sorted.windows(2).map(|w| w[1] - w[0]).collect();
    // The spacing that the distances are (near) multiples of. A half or a third of the spacing
    // fits as well or better, so take the biggest that fits nearly as well as the best.
    let fits = |c: f32| {
        diffs
            .iter()
            .filter(|&&d| (d / c).round() >= 1.0 && (d / c - (d / c).round()).abs() < 0.15)
            .count()
    };
    let best = diffs.iter().map(|&d| fits(d)).max()?;
    let rough = diffs
        .iter()
        .copied()
        .filter(|&d| fits(d) * 5 >= best * 3)
        .max_by(f32::total_cmp)?;
    // Sharpen it with the distances that are about one step.
    let steps: Vec<f32> = diffs
        .iter()
        .copied()
        .filter(|&d| (d / rough - 1.0).abs() < 0.15)
        .collect();
    let rough = steps.iter().sum::<f32>() / steps.len() as f32;
    // Number the positions, then fit. (`rough` is now close enough to count steps from the
    // first position, even far along. Counting step-by-step instead would let one stray mark
    // half a step out throw off the count for everything after it.)
    let mut numbered: Vec<(f32, f32)> = sorted
        .iter()
        .map(|&p| (((p - sorted[0]) / rough).round(), p))
        .collect();
    let mut edge = Edge::fit(&numbered, 0.0);
    // Stray marks would skew it; refit without the worst-fitting tenth.
    numbered.sort_by(|a, b| {
        let off = |p: &(f32, f32)| (p.1 - edge.at(p.0)).abs();
        off(a).total_cmp(&off(b))
    });
    numbered.truncate(numbered.len() - numbered.len() / 10);
    if numbered.len() >= 2 {
        edge = Edge::fit(&numbered, 0.0);
    }
    if edge.slope <= 0.0 {
        return None;
    }
    Some((edge.slope, edge.intercept.rem_euclid(edge.slope)))
}

/// Turn `glyphs` (each one assigned to a slot) into numbers. `same_number` says whether two
/// glyphs, adjacent in reading order, are digits of a single number.
fn read_numbers(digits: &[Glyph], same_number: impl Fn(&Glyph, &Glyph) -> bool) -> Vec<u16> {
    let mut numbers: Vec<u16> = vec![];
    for (i, g) in digits.iter().enumerate() {
        if i > 0 && same_number(&digits[i - 1], g) {
            let last = numbers.last_mut().unwrap();
            *last = last.saturating_mul(10).saturating_add(g.digit as u16);
        } else {
            numbers.push(g.digit as u16);
        }
    }
    numbers
}

/// The threshold that separates "next digit of the same number" from "next number", given the
/// distances between neighboring digits. The digits of one number are set tight, never as far
/// apart as `limit`; within that, look for a clear jump. If there's none, every digit is its own
/// number. `min_jump` is how much bigger the next-number distances must be.
pub fn split_threshold(mut distances: Vec<f32>, limit: f32, min_jump: f32) -> f32 {
    distances.sort_by(f32::total_cmp);
    let mut best = (min_jump, 0.0);
    for w in distances.windows(2) {
        let ratio = w[1] / w[0].max(0.1);
        if w[0] < limit && ratio > best.0 {
            best = (ratio, (w[0] * w[1]).sqrt().min(limit));
        }
    }
    best.1
}

/// Read the clues out of `glyphs`. `width` and `height` override the grid size; without them,
/// empty columns at the right (and rows at the bottom) are invisible.
pub fn arrange(
    glyphs: &[Glyph],
    width: Option<usize>,
    height: Option<usize>,
) -> anyhow::Result<ClueLayout> {
    use anyhow::{Context, bail};
    let h = median(glyphs.iter().map(|g| g.height).collect()).context("no digits found")?;

    let cols = find_block(glyphs, Axis::Vertical, h).context("couldn't find column clues")?;
    let rows = find_block(glyphs, Axis::Horizontal, h).context("couldn't find row clues")?;
    let (top, left) = (cols.edge, rows.edge);

    // Above the top edge and right of the left edge is column clues; the reverse is row clues.
    // The corner itself, and the grid, hold nothing of interest.
    let in_cols = |g: &Glyph| g.y < top.at(g.x) && g.x > left.at(g.y);
    let in_rows = |g: &Glyph| g.x < left.at(g.y) && g.y > top.at(g.x);
    let col_member = cols.members(glyphs, in_cols, h);
    let row_member = rows.members(glyphs, in_rows, h);

    // Cells are usually square, so if one direction has too few clues to measure, borrow the
    // other's measurement.
    let positions = |member: &[bool], pos: fn(&Glyph) -> f32| -> Vec<f32> {
        (0..glyphs.len())
            .filter(|&i| member[i])
            .map(|i| pos(&glyphs[i]))
            .collect()
    };
    let col_lattice = lattice(&positions(&col_member, |g| g.x), h);
    let row_lattice = lattice(&positions(&row_member, |g| g.y), h);
    let ((col_pitch, col_phase), (row_pitch, row_phase)) = match (col_lattice, row_lattice) {
        (Some(c), Some(r)) => (c, r),
        (Some(c), None) => {
            let (_, phase) = lattice(&positions(&row_member, |g| g.y), 0.0).unwrap_or((0.0, 0.0));
            (c, (c.0, phase))
        }
        (None, Some(r)) => {
            let (_, phase) = lattice(&positions(&col_member, |g| g.x), 0.0).unwrap_or((0.0, 0.0));
            ((r.0, phase), r)
        }
        (None, None) => bail!("couldn't measure the cell size"),
    };

    // Where the two edges cross:
    let corner_x = (left.intercept + left.slope * top.intercept) / (1.0 - left.slope * top.slope);
    let corner_y = top.at(corner_x);

    // Lattice positions are `phase + n * pitch`. The first cell's middle is half a cell past the
    // corner, plus the margin between the clues and the grid, which can be most of a cell when
    // the cells are small. (So a blank line at the start only shows if it's a whole cell
    // further.) Allow for the edge being a little off the other way, too.
    let first = |corner: f32, phase: f32, pitch: f32| -> i64 {
        ((corner + 0.5 * pitch - phase) / pitch - 0.15).ceil() as i64
    };
    let slot = |pos: f32, phase: f32, pitch: f32, corner: f32| -> i64 {
        ((pos - phase) / pitch).round() as i64 - first(corner, phase, pitch)
    };

    let mut roles = vec![Role::Ignored; glyphs.len()];
    let mut warnings = vec![];
    for (i, g) in glyphs.iter().enumerate() {
        let (role, s) = if col_member[i] {
            let s = slot(g.x, col_phase, col_pitch, corner_x);
            (Role::Col(s.max(0) as usize), s)
        } else if row_member[i] {
            let s = slot(g.y, row_phase, row_pitch, corner_y);
            (Role::Row(s.max(0) as usize), s)
        } else {
            continue;
        };
        if s < 0 {
            warnings.push(format!(
                "the {} at ({:.0}, {:.0}) is before the first cell",
                g.digit, g.x, g.y
            ));
            continue;
        }
        roles[i] = role;
    }

    let size =
        |found: usize, given: Option<usize>, what: &str, warnings: &mut Vec<String>| match given {
            Some(given) if given < found => {
                warnings.push(format!(
                    "found clues for {found} {what}, but was told {given}"
                ));
                found
            }
            Some(given) => given,
            None => found,
        };
    let found_cols = roles.iter().filter_map(|r| match r {
        Role::Col(c) => Some(c + 1),
        _ => None,
    });
    let found_rows = roles.iter().filter_map(|r| match r {
        Role::Row(r) => Some(r + 1),
        _ => None,
    });
    let width = size(
        found_cols.max().unwrap_or(0),
        width,
        "columns",
        &mut warnings,
    );
    let height = size(found_rows.max().unwrap_or(0), height, "rows", &mut warnings);

    let mut col_digits: Vec<Vec<Glyph>> = vec![vec![]; width];
    let mut row_digits: Vec<Vec<Glyph>> = vec![vec![]; height];
    for (g, role) in glyphs.iter().zip(&roles) {
        match role {
            Role::Col(c) => col_digits[*c].push(*g),
            Role::Row(r) => row_digits[*r].push(*g),
            Role::Ignored => {}
        }
    }

    // In a column, a number is the digits on one line (they're already confined to the column).
    let first_col = first(corner_x, col_phase, col_pitch) as f32;
    let col_clues: Vec<Vec<u16>> = col_digits
        .iter_mut()
        .enumerate()
        .map(|(c, digits)| {
            digits.sort_by(|a, b| {
                if (a.y - b.y).abs() < 0.5 * h {
                    a.x.total_cmp(&b.x)
                } else {
                    a.y.total_cmp(&b.y)
                }
            });
            // A two-digit number is centered in its column. If a pair is lopsided, one of them is
            // a speck that OCR took for a digit, so keep the one in the middle. Where "the
            // middle" is comes from the digits that are alone on their line, if there are any.
            let alone: Vec<f32> = (0..digits.len())
                .filter(|&i| {
                    let same_line = |j: usize| (digits[i].y - digits[j].y).abs() < 0.5 * h;
                    (i == 0 || !same_line(i - 1)) && (i + 1 == digits.len() || !same_line(i + 1))
                })
                .map(|i| digits[i].x)
                .collect();
            let center = median(alone).unwrap_or(col_phase + (c as f32 + first_col) * col_pitch);
            let mut i = 0;
            while i + 1 < digits.len() {
                let (a, b) = (digits[i], digits[i + 1]);
                let lone = i + 2 >= digits.len() || (digits[i + 2].y - b.y).abs() >= 0.5 * h;
                if (a.y - b.y).abs() < 0.5 * h && lone {
                    let off_center = ((a.x + b.x) / 2.0 - center).abs();
                    if off_center > 0.3 * (b.x - a.x) {
                        let speck = if (a.x - center).abs() < (b.x - center).abs() {
                            i + 1
                        } else {
                            i
                        };
                        warnings.push(format!(
                            "column {}: ignoring the off-center {} at ({:.0}, {:.0})",
                            c + 1,
                            digits[speck].digit,
                            digits[speck].x,
                            digits[speck].y
                        ));
                        digits.remove(speck);
                    }
                }
                i += 1;
            }
            read_numbers(digits, |a, b| (a.y - b.y).abs() < 0.5 * h)
        })
        .collect();

    // In a row, it's the spacing that tells. The rows all share a typeface, so decide what
    // "close" means from all of them together.
    for digits in &mut row_digits {
        digits.sort_by(|a, b| a.x.total_cmp(&b.x));
    }
    let threshold = split_threshold(
        row_digits
            .iter()
            .flat_map(|d| d.windows(2).map(|w| w[1].x - w[0].x))
            .collect(),
        // (Center to center, which is a glyph width more than the gap.)
        0.85 * h,
        1.25,
    );
    let row_clues: Vec<Vec<u16>> = row_digits
        .iter()
        .map(|digits| read_numbers(digits, |a, b| b.x - a.x < threshold))
        .collect();

    let mut check = |clues: &[Vec<u16>], len: usize, what: &str| {
        for (i, clue) in clues.iter().enumerate() {
            let what = format!("{what} {}", i + 1);
            if clue.iter().any(|&n| n >= 100) {
                warnings.push(format!(
                    "{what}: {clue:?} has a number with too many digits"
                ));
            }
            if clue.contains(&0) && clue.len() > 1 {
                warnings.push(format!("{what}: {clue:?} has a 0 among other numbers"));
            }
            let needed = clue.iter().map(|&n| n as usize).sum::<usize>() + clue.len().max(1) - 1;
            if needed > len {
                warnings.push(format!("{what}: {clue:?} doesn't fit in {len} cells"));
            }
        }
    };
    check(&col_clues, height, "column");
    check(&row_clues, width, "row");

    // A clue of "0" is how some puzzles write an empty line.
    let tidy = |clues: Vec<Vec<u16>>| -> Vec<Vec<u16>> {
        clues
            .into_iter()
            .map(|c| c.into_iter().filter(|&n| n != 0).collect())
            .collect()
    };
    let (col_clues, row_clues) = (tidy(col_clues), tidy(row_clues));

    let total = |clues: &[Vec<u16>]| clues.iter().flatten().map(|&n| n as usize).sum::<usize>();
    if total(&col_clues) != total(&row_clues) {
        warnings.push(format!(
            "the column clues fill {} cells, but the row clues fill {}",
            total(&col_clues),
            total(&row_clues)
        ));
    }

    let first_row = first(corner_y, row_phase, row_pitch) as f32;
    Ok(ClueLayout {
        rows: row_clues,
        cols: col_clues,
        roles,
        grid_top: top,
        grid_left: left,
        col_centers: (0..width)
            .map(|c| col_phase + (c as f32 + first_col) * col_pitch)
            .collect(),
        row_centers: (0..height)
            .map(|r| row_phase + (r as f32 + first_row) * row_pitch)
            .collect(),
        col_pitch,
        row_pitch,
        glyph_height: h,
        edges_on_lines: false,
        warnings,
    })
}

/// Clues for a picture, as a human read them, for measuring how well `arrange` (and the OCR
/// feeding it) did. Kept in a simple text format, meant for editing by hand:
///
/// ```text
/// columns
/// 1 2
/// 0
/// rows
/// 3
/// ```
///
/// One line per lane, `0` for an empty one, and `?` for a clue that can't be read. Blank lines
/// and `#` comments are ignored.
#[derive(Clone, Debug, PartialEq)]
pub struct Expected {
    pub cols: Vec<Vec<u16>>,
    pub rows: Vec<Vec<u16>>,
    /// False until a human has checked them, which they say by deleting the line containing
    /// `UNCHECKED`.
    pub checked: bool,
}

impl Expected {
    pub const UNCHECKED: &str = "UNCHECKED";

    /// The file for a human to correct: what was read from `source`, marked unchecked.
    pub fn render(cols: &[Vec<u16>], rows: &[Vec<u16>], source: &str) -> String {
        let mut text = format!(
            "# {}: what ocr-clues read from {source}. Correct it, then delete this line.\n",
            Expected::UNCHECKED
        );
        for (name, lanes) in [("columns", cols), ("rows", rows)] {
            text.push_str(name);
            text.push('\n');
            for lane in lanes {
                if lane.is_empty() {
                    text.push('0');
                } else {
                    let numbers: Vec<String> = lane.iter().map(|&n| clue_text(n)).collect();
                    text.push_str(&numbers.join(" "));
                }
                text.push('\n');
            }
        }
        text
    }

    pub fn parse(text: &str) -> anyhow::Result<Expected> {
        use anyhow::{Context, bail};
        let mut expected = Expected {
            cols: vec![],
            rows: vec![],
            checked: true,
        };
        let mut section: Option<&mut Vec<Vec<u16>>> = None;
        for (i, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.starts_with('#') {
                if line.contains(Expected::UNCHECKED) {
                    expected.checked = false;
                }
                continue;
            }
            match line {
                "" => {}
                "columns" => section = Some(&mut expected.cols),
                "rows" => section = Some(&mut expected.rows),
                _ => {
                    let Some(lanes) = section.as_mut() else {
                        bail!("line {}: expected \"columns\" or \"rows\" first", i + 1);
                    };
                    let lane = line
                        .split_whitespace()
                        .map(|n| {
                            if n == "?" {
                                Ok(BLOTTED)
                            } else {
                                n.parse::<u16>()
                            }
                        })
                        .collect::<Result<Vec<u16>, _>>()
                        .with_context(|| format!("line {}: expected numbers", i + 1))?;
                    lanes.push(lane.into_iter().filter(|&n| n != 0).collect());
                }
            }
        }
        Ok(expected)
    }

    /// How far `cols` and `rows` are from these.
    pub fn score(&self, cols: &[Vec<u16>], rows: &[Vec<u16>]) -> Score {
        let mut score = Score::default();
        for (name, expected, actual) in [("column", &self.cols, cols), ("row", &self.rows, rows)] {
            if expected.len() != actual.len() {
                score.mistakes.push(format!(
                    "expected {} {name}s, got {}",
                    expected.len(),
                    actual.len()
                ));
            }
            for i in 0..expected.len().max(actual.len()) {
                let none = vec![];
                let want = expected.get(i).unwrap_or(&none);
                let got = actual.get(i).unwrap_or(&none);
                score.lanes += 1;
                score.numbers += want.len();
                let missing = i >= expected.len() || i >= actual.len();
                if want == got && !missing {
                    score.lanes_right += 1;
                } else if missing {
                    // (Already reported, with the count.)
                    score.numbers_wrong += want.len().max(got.len());
                } else {
                    score.numbers_wrong += edit_distance(want, got);
                    let show = |lane: &[u16]| {
                        let numbers: Vec<String> = lane.iter().map(|&n| clue_text(n)).collect();
                        if numbers.is_empty() {
                            "(empty)".to_string()
                        } else {
                            numbers.join(" ")
                        }
                    };
                    score.mistakes.push(format!(
                        "{name} {}: expected {}, got {}",
                        i + 1,
                        show(want),
                        show(got)
                    ));
                }
            }
        }
        score
    }
}

/// The result of comparing clues to `Expected` ones.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Score {
    pub lanes: usize,
    pub lanes_right: usize,
    /// Numbers in the expected clues
    pub numbers: usize,
    /// Numbers that would have to be added, removed, or changed to make the clues right
    pub numbers_wrong: usize,
    pub mistakes: Vec<String>,
}

/// The number of insertions, deletions, and substitutions to turn `a` into `b`.
fn edit_distance(a: &[u16], b: &[u16]) -> usize {
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, x) in a.iter().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, y) in b.iter().enumerate() {
            let substitute = diagonal + usize::from(x != y);
            diagonal = row[j + 1];
            row[j + 1] = substitute.min(row[j] + 1).min(row[j + 1] + 1);
        }
    }
    row[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How a synthetic puzzle picture is laid out. All in pixels.
    struct Style {
        cell: f32,
        /// Glyph height
        h: f32,
        /// Center-to-center distance between digits of one number
        advance: f32,
        /// Center-to-center distance between stacked column clues
        line: f32,
        /// Distance between neighboring row clues, edge to edge
        gap: f32,
        /// Clockwise rotation of the whole picture, in radians
        tilt: f32,
    }

    const ROOMY: Style = Style {
        cell: 40.0,
        h: 24.0,
        advance: 14.0,
        line: 34.0,
        gap: 16.0,
        tilt: 0.0,
    };

    /// The clues of a picture given as rows of `#` and `.`.
    fn clues(picture: &[&str]) -> (Vec<Vec<u16>>, Vec<Vec<u16>>) {
        let runs = |cells: Vec<bool>| -> Vec<u16> {
            let mut runs = vec![];
            let mut run = 0;
            for filled in cells.into_iter().chain([false]) {
                if filled {
                    run += 1;
                } else if run > 0 {
                    runs.push(run);
                    run = 0;
                }
            }
            runs
        };
        let grid: Vec<Vec<bool>> = picture
            .iter()
            .map(|row| row.chars().map(|c| c == '#').collect())
            .collect();
        let rows = grid.iter().map(|row| runs(row.clone())).collect();
        let cols = (0..grid[0].len())
            .map(|c| runs(grid.iter().map(|row| row[c]).collect()))
            .collect();
        (rows, cols)
    }

    /// Lay out the clues as a picture would show them: column clues stacked above the grid,
    /// centered in their columns; row clues left of it, right-aligned.
    fn render(rows: &[Vec<u16>], cols: &[Vec<u16>], style: &Style) -> Vec<Glyph> {
        let (corner_x, corner_y) = (300.0, 300.0);
        let mut glyphs = vec![];
        let mut number = |n: u16, center_x: f32, y: f32| {
            let digits: Vec<u8> = n.to_string().bytes().map(|b| b - b'0').collect();
            let first = center_x - style.advance * (digits.len() - 1) as f32 / 2.0;
            for (i, &digit) in digits.iter().enumerate() {
                let (x, y) = (first + i as f32 * style.advance, y);
                // Rotate about the origin.
                let (sin, cos) = style.tilt.sin_cos();
                glyphs.push(Glyph {
                    digit,
                    x: x * cos - y * sin,
                    y: x * sin + y * cos,
                    width: style.h * 0.55,
                    height: style.h,
                });
            }
        };
        for (c, clue) in cols.iter().enumerate() {
            let x = corner_x + (c as f32 + 0.5) * style.cell;
            for (k, &n) in clue.iter().rev().enumerate() {
                number(n, x, corner_y - 6.0 - style.h / 2.0 - k as f32 * style.line);
            }
        }
        for (r, clue) in rows.iter().enumerate() {
            let y = corner_y + (r as f32 + 0.5) * style.cell;
            let mut right = corner_x - 6.0;
            for &n in clue.iter().rev() {
                let width = style.advance * (n.to_string().len() - 1) as f32 + style.h * 0.55;
                number(n, right - width / 2.0, y);
                right -= width + style.gap;
            }
        }
        glyphs
    }

    fn assert_reads(picture: &[&str], style: &Style, extra: &[Glyph]) {
        let (rows, cols) = clues(picture);
        let mut glyphs = render(&rows, &cols, style);
        glyphs.extend_from_slice(extra);
        let layout = arrange(&glyphs, Some(cols.len()), Some(rows.len())).unwrap();
        assert_eq!(layout.cols, cols, "columns");
        assert_eq!(layout.rows, rows, "rows");
        assert_eq!(layout.warnings, Vec::<String>::new());
    }

    const HEART: &[&str] = &[
        ".##...##.",
        "####.####",
        "#########",
        "#########",
        ".#######.",
        "..#####..",
        "...###...",
        "....#....",
    ];

    #[test]
    fn plain() {
        assert_reads(HEART, &ROOMY, &[]);
    }

    #[test]
    fn packed_rows() {
        // Every digit spaced the same, as some apps do. With no two-digit clues, that's fine.
        let style = Style {
            gap: 24.0 - 0.55 * 24.0,
            ..ROOMY
        };
        assert_reads(HEART, &style, &[]);
    }

    #[test]
    fn two_digit_numbers() {
        let picture = &[
            "############",
            "############",
            "#.#.#.#.#.#.",
            "............",
            "##.#########",
            "..#.........",
            "#.#.#.#.#.#.",
            "#.#.#.#.#.#.",
            "#.#.#.#.#.#.",
            "#.#.#.#.#.#.",
            "#.#.#.#.#.#.",
            "#.#.#.#.#.#.",
        ];
        assert_reads(picture, &ROOMY, &[]);
        // Cells about as wide as a two-digit number, so the digits of neighboring columns are
        // evenly spaced. ("10" and "2" next to each other look like "1 0 2".)
        let tight = Style {
            cell: 28.0,
            line: 30.0,
            ..ROOMY
        };
        assert_reads(picture, &tight, &[]);
    }

    #[test]
    fn empty_lines() {
        // Blank lines inside, and at the start (the corner gives that away).
        let picture = &[
            ".........",
            "..#.#..#.",
            "..##..##.",
            ".........",
            "..##.###.",
            "..#.#.#..",
        ];
        assert_reads(picture, &ROOMY, &[]);
    }

    #[test]
    fn missing_trailing_lines_need_to_be_given() {
        let picture = &["#..", "##.", "..."];
        let (rows, cols) = clues(picture);
        let glyphs = render(&rows, &cols, &ROOMY);
        let layout = arrange(&glyphs, None, None).unwrap();
        assert_eq!(layout.cols, vec![vec![2], vec![1]]);
        assert_eq!(layout.rows, vec![vec![1], vec![2]]);
    }

    #[test]
    fn noise() {
        let speck = |digit, x, y| Glyph {
            digit,
            x,
            y,
            width: 13.0,
            height: 24.0,
        };
        assert_reads(
            HEART,
            &ROOMY,
            &[
                // A title, in the corner above the row clues
                speck(4, 150.0, 100.0),
                speck(2, 164.0, 100.0),
                // A score, far above the column clues
                speck(9, 450.0, 20.0),
                // Marks in the grid
                speck(1, 330.0, 345.0),
                speck(8, 420.0, 460.0),
                speck(8, 460.0, 460.0),
                // Right next to the last row clue, but in the grid
                speck(1, 318.0, 580.0),
            ],
        );
    }

    #[test]
    fn tilted() {
        let style = Style {
            tilt: 0.01,
            ..ROOMY
        };
        assert_reads(HEART, &style, &[]);
    }

    #[test]
    fn off_center_speck() {
        let (rows, cols) = clues(HEART);
        let mut glyphs = render(&rows, &cols, &ROOMY);
        // Next to the bottom clue of the first column, at (320, 282).
        glyphs.push(Glyph {
            digit: 1,
            x: 302.0,
            y: 282.0,
            width: 8.0,
            height: 20.0,
        });
        let layout = arrange(&glyphs, None, None).unwrap();
        assert_eq!(layout.cols, cols);
        assert_eq!(layout.warnings.len(), 1);
    }

    #[test]
    fn split_thresholds() {
        // All alike: no two-digit numbers.
        assert_eq!(
            split_threshold(vec![24.0, 24.5, 23.0, 25.0], 20.0, 1.25),
            0.0
        );
        // Two clear groups.
        let t = split_threshold(vec![14.0, 30.0, 31.0, 13.5, 29.0], 20.0, 1.25);
        assert!(14.0 < t && t < 29.0);
        // Two groups, but both too spread out to be digits of one number.
        assert_eq!(split_threshold(vec![30.0, 60.0, 61.0], 20.0, 1.25), 0.0);
    }

    #[test]
    fn expected_round_trip() {
        let cols = vec![vec![1, 2], vec![], vec![10]];
        let rows = vec![vec![3], vec![]];
        let text = Expected::render(&cols, &rows, "x.png");
        let expected = Expected::parse(&text).unwrap();
        assert_eq!((&expected.cols, &expected.rows), (&cols, &rows));
        assert!(!expected.checked);

        // Once a human deletes the marker (and maybe leaves some blank lines around):
        let text: String = text.lines().skip(1).map(|l| format!("{l}\n\n")).collect();
        let expected = Expected::parse(&text).unwrap();
        assert_eq!((&expected.cols, &expected.rows), (&cols, &rows));
        assert!(expected.checked);
    }

    #[test]
    fn scoring() {
        let expected = Expected {
            cols: vec![vec![1, 2], vec![3]],
            rows: vec![vec![1, 11], vec![2]],
            checked: true,
        };
        let score = expected.score(&[vec![1, 2], vec![3]], &[vec![1, 1, 1], vec![2], vec![]]);
        assert_eq!(score.lanes, 5);
        assert_eq!(score.lanes_right, 3);
        assert_eq!(score.numbers, 6);
        // "11" became "1 1": one substitution, one insertion.
        assert_eq!(score.numbers_wrong, 2);
        assert_eq!(score.mistakes.len(), 2); // the bad row, and the row count
        assert_eq!(edit_distance(&[1, 2, 3], &[1, 3]), 1);
        assert_eq!(edit_distance(&[], &[4, 4]), 2);
    }

    #[test]
    fn lattices() {
        let (pitch, phase) = lattice(&[105.0, 145.0, 147.0, 225.0, 266.0, 545.0], 24.0).unwrap();
        assert!((pitch - 40.0).abs() < 0.5, "{pitch}");
        assert!((phase - 25.0).abs() < 2.0, "{phase}");
    }
}
