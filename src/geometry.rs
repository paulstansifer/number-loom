//! Puzzle shapes, and the lanes (lines of cells) that clues apply to.
//!
//! A [`LaneMap`] captures the part of a shape the solver needs:
//!   * a dense numbering of the cells, `0..cell_count()`,
//!   * a list of [`Lane`]s, each an ordered list of cell indices, grouped into *families*
//!     (a family is one direction: rows, columns, `/` lines, ...),
//!   * for each cell, which lanes contain it and at which position.
//!
//! Square puzzles have two families (rows, then columns). Triangular ("triddler") puzzles have
//! three. Everything above the lane level is geometry-agnostic.
//!
//! # Triangular coordinates
//!
//! A triangular cell is a triple `(a, b, c)` with `a - b + c` equal to `-1` (▲) or `0` (▼).
//! The three families are exactly "`a` constant" (horizontal rows), "`b` constant" (`/` lines),
//! and "`c` constant" (`\` lines), so each cell is in exactly one lane per family.
//!
//! An outline is just six bounds, one per coordinate per side. Every such box is a valid puzzle
//! and every valid puzzle is such a box: clues only make sense when every line is contiguous,
//! which forces the region to be convex, which makes it an intersection of six half-planes.
//! Resizing is therefore ±1 on one of six integers, and can never produce an invalid shape.
//!
//! Note that unlike a square grid, **two lanes from different families may share two cells**:
//! a ▲ and the ▼ to its right lie in the same row *and* the same `/` line.

use serde::{Deserialize, Serialize};

/// A triangular cell. `a` indexes its row, `b` its `/` line, `c` its `\` line.
#[derive(PartialEq, Eq, Clone, Copy, Debug, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TriCoord {
    pub a: i32,
    pub b: i32,
    pub c: i32,
}

impl TriCoord {
    pub fn new(a: i32, b: i32, c: i32) -> TriCoord {
        TriCoord { a, b, c }
    }

    /// Only two of the four possible values of `a - b + c` denote a real cell.
    pub fn is_valid(&self) -> bool {
        matches!(self.a - self.b + self.c, -1 | 0)
    }

    /// True for ▲, false for ▼.
    pub fn points_up(&self) -> bool {
        self.a - self.b + self.c == -1
    }

    /// Where to draw this cell: `(row, p)`, where the triangle spans horizontal half-units
    /// `[p, p + 2]` and the row occupies vertical band `[a, a + 1]`.
    pub fn to_row_and_half_unit(&self) -> (i32, i32) {
        let p = if self.points_up() {
            2 * self.b - self.a
        } else {
            2 * self.b + 1 - self.a
        };
        (self.a, p)
    }

    /// The inverse of `to_row_and_half_unit`.
    ///
    /// A triangle in row `a` at half-unit `p` points up exactly when `p + a` is even, because
    /// `p + a` is `2b` for ▲ and `2b + 1` for ▼.
    pub fn from_row_and_half_unit(a: i32, p: i32) -> TriCoord {
        if (p + a).rem_euclid(2) == 0 {
            let b = (p + a) / 2;
            TriCoord::new(a, b, b - a - 1)
        } else {
            let b = (p + a - 1) / 2;
            TriCoord::new(a, b, b - a)
        }
    }

    fn step(&self, dir: Direction) -> TriCoord {
        let TriCoord { a, b, c } = *self;
        let up = self.points_up();
        match (dir, up) {
            (Direction::Right, true) => TriCoord::new(a, b, c + 1),
            (Direction::Right, false) => TriCoord::new(a, b + 1, c),
            (Direction::Left, true) => TriCoord::new(a, b - 1, c),
            (Direction::Left, false) => TriCoord::new(a, b, c - 1),
            (Direction::DownLeft, true) => TriCoord::new(a + 1, b, c),
            (Direction::DownLeft, false) => TriCoord::new(a, b, c - 1),
            (Direction::UpRight, true) => TriCoord::new(a, b, c + 1),
            (Direction::UpRight, false) => TriCoord::new(a - 1, b, c),
            (Direction::UpLeft, true) => TriCoord::new(a, b - 1, c),
            (Direction::UpLeft, false) => TriCoord::new(a - 1, b, c),
            (Direction::DownRight, true) => TriCoord::new(a + 1, b, c),
            (Direction::DownRight, false) => TriCoord::new(a, b + 1, c),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Direction {
    Left,
    Right,
    UpRight,
    DownLeft,
    UpLeft,
    DownRight,
}

/// The six bounds that describe a triangular puzzle's outline. Ranges are inclusive.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, Serialize, Deserialize)]
pub struct Outline {
    pub a: (i32, i32),
    pub b: (i32, i32),
    pub c: (i32, i32),
}

impl Outline {
    /// A regular hexagon with `side` cells along each edge; `6 * side²` cells in all.
    pub fn hexagon(side: i32) -> Outline {
        Outline {
            a: (0, 2 * side - 1),
            b: (0, 2 * side - 1),
            c: (-side, side - 1),
        }
    }

    /// Translate to a canonical position, with `a₀ = c₀ = 0`.
    ///
    /// `(a, b) += k` and `(b, c) += k` both leave `a - b + c` alone, so they slide an outline
    /// around without changing its shape at all. Canonicalizing means two descriptions of the
    /// same shape compare equal, which they otherwise wouldn't.
    pub fn normalized(&self) -> Outline {
        let (da, dc) = (self.a.0, self.c.0);
        Outline {
            a: (0, self.a.1 - da),
            b: (self.b.0 - da - dc, self.b.1 - da - dc),
            c: (0, self.c.1 - dc),
        }
    }

    pub fn contains(&self, t: TriCoord) -> bool {
        t.is_valid()
            && (self.a.0..=self.a.1).contains(&t.a)
            && (self.b.0..=self.b.1).contains(&t.b)
            && (self.c.0..=self.c.1).contains(&t.c)
    }

    /// All cells, ordered by row and then left-to-right within the row.
    pub fn cells(&self) -> Vec<TriCoord> {
        let mut res = vec![];
        for a in self.a.0..=self.a.1 {
            for b in self.b.0..=self.b.1 {
                for c in self.c.0..=self.c.1 {
                    let t = TriCoord::new(a, b, c);
                    if self.contains(t) {
                        res.push(t);
                    }
                }
            }
        }
        // `(a, b, c)` ascending is exactly row-major, left-to-right.
        res.sort();
        res
    }

    /// Grow (`delta > 0`) or shrink the given side. Returns `None` if the puzzle would be empty.
    pub fn resized(&self, side: Side, delta: i32) -> Option<Outline> {
        let mut res = *self;
        match side {
            Side::Top => res.a.0 -= delta,
            Side::Bottom => res.a.1 += delta,
            Side::UpperLeft => res.b.0 -= delta,
            Side::LowerRight => res.b.1 += delta,
            Side::LowerLeft => res.c.0 -= delta,
            Side::UpperRight => res.c.1 += delta,
        }
        if res.a.0 > res.a.1 || res.b.0 > res.b.1 || res.c.0 > res.c.1 {
            return None;
        }
        // A box can be non-empty in every coordinate yet still contain no valid cell.
        if res.cells().is_empty() {
            None
        } else {
            Some(res)
        }
    }
}

/// How many lanes fall into each of webpbn's six clue sets, in the order the format lists them.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct ClueSetCounts {
    pub topleft: usize,
    pub bottomleft: usize,
    pub top: usize,
    pub topright: usize,
    pub bottom: usize,
    pub bottomright: usize,
}

impl Outline {
    /// Recover an outline from nothing but the number of clue-lines in each of webpbn's six sets.
    ///
    /// This works because the six bounds have only four degrees of freedom: `(a, b) += k` and
    /// `(b, c) += k` both leave `a - b + c` alone, so the shape is unchanged by them. Pinning
    /// `a₀ = c₀ = 0` uses up both, the three family sizes give `a₁`, `b₁` and `c₁` relative to
    /// their minima, and `b₀` is the single remaining unknown — so we can simply try every
    /// plausible `b₀` and keep the one that reproduces all six counts.
    ///
    /// A file whose counts match no outline is malformed, and says so rather than guessing.
    pub fn from_clue_set_counts(counts: ClueSetCounts) -> anyhow::Result<Outline> {
        let a_size = counts.topleft + counts.bottomleft;
        let b_size = counts.top + counts.topright;
        let c_size = counts.bottom + counts.bottomright;

        if a_size == 0 || b_size == 0 || c_size == 0 {
            anyhow::bail!(
                "a triddler needs at least one clue line in each of the three directions"
            );
        }

        let span = (a_size + b_size + c_size) as i32;
        for b0 in -span..=span {
            let candidate = Outline {
                a: (0, a_size as i32 - 1),
                b: (b0, b0 + b_size as i32 - 1),
                c: (0, c_size as i32 - 1),
            };
            if candidate.cells().is_empty() {
                continue;
            }
            if Geometry::<Tri>::new(candidate).clue_set_counts() == counts {
                return Ok(candidate);
            }
        }

        anyhow::bail!(
            "no triddler outline has {} clue lines split {}/{}, {} split {}/{}, and {} split {}/{}",
            a_size,
            counts.topleft,
            counts.bottomleft,
            b_size,
            counts.top,
            counts.topright,
            c_size,
            counts.bottom,
            counts.bottomright,
        )
    }
}

/// The six sides of a triangular outline, each independently resizable.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    Top,
    UpperRight,
    LowerRight,
    Bottom,
    LowerLeft,
    UpperLeft,
}

impl Side {
    pub fn all() -> [Side; 6] {
        [
            Side::Top,
            Side::UpperRight,
            Side::LowerRight,
            Side::Bottom,
            Side::LowerLeft,
            Side::UpperLeft,
        ]
    }
}

/// Which of webpbn's six `<clues type=...>` sets a lane's clues belong to. Determined by which
/// bound stops the lane at the end the clues are written from.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ClueSet {
    TopLeft,
    BottomLeft,
    Top,
    TopRight,
    Bottom,
    BottomRight,
}

#[derive(Clone, PartialEq, Eq, Debug, Hash, Serialize, Deserialize)]
pub enum Shape {
    Square { width: usize, height: usize },
    Triangular(Outline),
}

/// One line of cells that a single clue-list applies to.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Lane {
    pub family: usize,
    /// Cell indices, in the order the clues are read.
    pub cells: Vec<u32>,
    /// Only meaningful for triangular puzzles.
    pub clue_set: Option<ClueSet>,
}

/// Where a cell sits within one of the lanes containing it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Membership {
    pub lane: u32,
    pub position: u32,
}

/// The part of a puzzle's shape that the solver needs: how many cells there are, which lanes
/// they form, and which lanes each cell belongs to.
///
/// Deliberately free of coordinates. `grid_solve` works entirely in terms of this, which is why
/// solving is identical for every shape and needs no notion of a cell's position.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct LaneMap {
    cell_count: usize,
    lanes: Vec<Lane>,
    /// `family_starts[f]..family_starts[f + 1]` indexes `lanes` for family `f`.
    family_starts: Vec<usize>,
    /// Indexed by cell; one entry per family.
    memberships: Vec<Vec<Membership>>,
}

impl LaneMap {
    fn new(cell_count: usize, lanes: Vec<Lane>, family_starts: Vec<usize>) -> LaneMap {
        let mut memberships = vec![vec![]; cell_count];
        for (lane_idx, lane) in lanes.iter().enumerate() {
            for (position, cell) in lane.cells.iter().enumerate() {
                memberships[*cell as usize].push(Membership {
                    lane: lane_idx as u32,
                    position: position as u32,
                });
            }
        }

        let res = LaneMap {
            cell_count,
            lanes,
            family_starts,
            memberships,
        };
        debug_assert!(res.each_cell_is_in_one_lane_per_family());
        res
    }

    fn each_cell_is_in_one_lane_per_family(&self) -> bool {
        self.memberships.iter().all(|m| {
            let mut families: Vec<usize> = m
                .iter()
                .map(|m| self.lanes[m.lane as usize].family)
                .collect();
            families.sort();
            families.dedup();
            families.len() == self.family_count() && m.len() == self.family_count()
        })
    }

    pub fn cell_count(&self) -> usize {
        self.cell_count
    }

    pub fn family_count(&self) -> usize {
        self.family_starts.len() - 1
    }

    pub fn lanes(&self) -> &[Lane] {
        &self.lanes
    }

    pub fn lane(&self, idx: usize) -> &Lane {
        &self.lanes[idx]
    }

    pub fn lane_count(&self) -> usize {
        self.lanes.len()
    }

    /// The range of lane indices belonging to `family`.
    pub fn family(&self, family: usize) -> std::ops::Range<usize> {
        self.family_starts[family]..self.family_starts[family + 1]
    }

    /// Every lane containing this cell, one per family.
    pub fn memberships(&self, cell: u32) -> &[Membership] {
        &self.memberships[cell as usize]
    }

    /// How many lanes fall into each webpbn clue set. Meaningless for square puzzles.
    pub fn clue_set_counts(&self) -> ClueSetCounts {
        let mut res = ClueSetCounts::default();
        for lane in &self.lanes {
            match lane.clue_set {
                Some(ClueSet::TopLeft) => res.topleft += 1,
                Some(ClueSet::BottomLeft) => res.bottomleft += 1,
                Some(ClueSet::Top) => res.top += 1,
                Some(ClueSet::TopRight) => res.topright += 1,
                Some(ClueSet::Bottom) => res.bottom += 1,
                Some(ClueSet::BottomRight) => res.bottomright += 1,
                None => {}
            }
        }
        res
    }

    /// Lane indices in the given clue set, in the order webpbn lists them.
    pub fn lanes_in_clue_set(&self, clue_set: ClueSet) -> Vec<usize> {
        (0..self.lanes.len())
            .filter(|i| self.lanes[*i].clue_set == Some(clue_set))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The worked example from `webpbn_tridder.md`: 16 cells in rows of 5, 6, 5.
    fn doc_example() -> Outline {
        Outline {
            a: (0, 2),
            b: (1, 3),
            c: (-1, 2),
        }
    }

    /// Label cells `A`..`P` the way the doc does, so lanes can be compared to it directly.
    fn spell(lane: &Lane) -> String {
        lane.cells
            .iter()
            .map(|c| (b'A' + *c as u8) as char)
            .collect()
    }

    #[test]
    fn doc_example_matches_webpbn() {
        let geo = Geometry::<Tri>::new(doc_example());
        assert_eq!(geo.cell_count(), 16);

        // The doc writes the solution as `/ABCDE\`, `/FGHIJK/`, `\LMNOP/`.
        let rows: Vec<String> = geo.family(0).map(|i| spell(geo.lane(i))).collect();
        assert_eq!(rows, vec!["ABCDE", "FGHIJK", "LMNOP"]);

        let slashes: Vec<String> = geo.family(1).map(|i| spell(geo.lane(i))).collect();
        assert_eq!(slashes, vec!["BAGFL", "DCIHNM", "EKJPO"]);

        let backslashes: Vec<String> = geo.family(2).map(|i| spell(geo.lane(i))).collect();
        // `\` lines read top-left to bottom-right, the opposite of the end their clues are
        // labelled from. See the note in `Geometry::<Tri>::new`.
        assert_eq!(backslashes, vec!["FLM", "AGHNO", "BCIJP", "DEK"]);
    }

    #[test]
    fn doc_example_clue_set_split() {
        let geo = Geometry::<Tri>::new(doc_example());
        let count = |cs: ClueSet| {
            geo.lane_map()
                .lanes()
                .iter()
                .filter(|l| l.clue_set == Some(cs))
                .count()
        };
        // The doc's example has: topleft 2, bottomleft 1, top 2, topright 1, bottom 2,
        // bottomright 2.
        assert_eq!(count(ClueSet::TopLeft), 2);
        assert_eq!(count(ClueSet::BottomLeft), 1);
        assert_eq!(count(ClueSet::Top), 2);
        assert_eq!(count(ClueSet::TopRight), 1);
        assert_eq!(count(ClueSet::Bottom), 2);
        assert_eq!(count(ClueSet::BottomRight), 2);
    }

    #[test]
    fn every_family_covers_every_cell_exactly_once() {
        for outline in [
            doc_example(),
            Outline::hexagon(3),
            Outline {
                a: (0, 3),
                b: (0, 3),
                c: (-3, 0),
            },
        ] {
            let geo = Geometry::<Tri>::new(outline);
            for family in 0..geo.family_count() {
                let mut seen = std::collections::HashSet::new();
                for lane in geo.family(family) {
                    for cell in &geo.lane(lane).cells {
                        assert!(seen.insert(*cell), "family {family} covers a cell twice");
                    }
                }
                assert_eq!(seen.len(), geo.cell_count(), "family {family} misses cells");
            }
        }
    }

    #[test]
    fn hexagons_have_six_n_squared_cells() {
        for side in 1..=5 {
            let geo = Geometry::<Tri>::new(Outline::hexagon(side));
            assert_eq!(geo.cell_count() as i32, 6 * side * side);
            // A regular hexagon splits every family evenly between its two clue sets.
            assert_eq!(geo.family(0).len() as i32, 2 * side);
            assert_eq!(geo.family(1).len() as i32, 2 * side);
            assert_eq!(geo.family(2).len() as i32, 2 * side);
        }
    }

    #[test]
    fn resizing_any_side_stays_valid() {
        let start = doc_example();
        for side in Side::all() {
            for delta in [1, -1] {
                let resized = start.resized(side, delta).expect("still non-empty");
                let geo = Geometry::<Tri>::new(resized);
                assert!(geo.cell_count() > 0);
                // `assemble` debug-asserts the one-lane-per-family invariant, so merely
                // building it is the check.
                assert_eq!(geo.family_count(), 3);
            }
        }
    }

    #[test]
    fn shrinking_to_nothing_is_refused() {
        let mut outline = Outline::hexagon(1);
        // Keep shrinking the same side; eventually it must refuse rather than go empty.
        let mut refused = false;
        for _ in 0..5 {
            match outline.resized(Side::Top, -1) {
                Some(smaller) => outline = smaller,
                None => {
                    refused = true;
                    break;
                }
            }
        }
        assert!(refused);
    }

    #[test]
    fn recovers_the_doc_example_outline_from_its_clue_counts() {
        // Exactly the six numbers webpbn's worked example implies.
        let counts = ClueSetCounts {
            topleft: 2,
            bottomleft: 1,
            top: 2,
            topright: 1,
            bottom: 2,
            bottomright: 2,
        };
        let recovered = Outline::from_clue_set_counts(counts).unwrap();

        // Recovery is only unique up to the two translations, so compare the shapes, not the
        // bounds: same cell count, same lane structure, same clue-set split.
        let geo = Geometry::<Tri>::new(recovered);
        assert_eq!(geo.cell_count(), 16);
        assert_eq!(geo.clue_set_counts(), counts);

        let rows: Vec<usize> = geo.family(0).map(|i| geo.lane(i).cells.len()).collect();
        assert_eq!(rows, vec![5, 6, 5]);
    }

    #[test]
    fn clue_counts_round_trip_for_many_outlines() {
        let mut outlines = vec![];
        for side in 1..=4 {
            outlines.push(Outline::hexagon(side));
        }
        for a1 in 1..5 {
            for b1 in 1..5 {
                for c0 in -3..1 {
                    outlines.push(Outline {
                        a: (0, a1),
                        b: (0, b1),
                        c: (c0, c0 + 3),
                    });
                }
            }
        }

        let mut checked = 0;
        for outline in outlines {
            let geo = Geometry::<Tri>::new(outline);
            if geo.cell_count() == 0 {
                continue;
            }
            let counts = geo.clue_set_counts();
            let recovered = Outline::from_clue_set_counts(counts)
                .unwrap_or_else(|e| panic!("{outline:?} -> {counts:?}: {e}"));

            // Same shape, even if the bounds are a translation apart.
            let recovered_geo = Geometry::<Tri>::new(recovered);
            assert_eq!(recovered_geo.cell_count(), geo.cell_count());
            assert_eq!(recovered_geo.clue_set_counts(), counts);
            for family in 0..3 {
                let original: Vec<usize> = geo
                    .family(family)
                    .map(|i| geo.lane(i).cells.len())
                    .collect();
                let round_tripped: Vec<usize> = recovered_geo
                    .family(family)
                    .map(|i| recovered_geo.lane(i).cells.len())
                    .collect();
                assert_eq!(original, round_tripped, "{outline:?} family {family}");
            }
            checked += 1;
        }
        assert!(checked > 50, "only checked {checked} outlines");
    }

    /// The Olsak solver documents two identities its hexagon sides must satisfy, where
    /// A=topleft, B=bottomleft, C=bottom, D=bottomright, E=topright, F=top (labelled
    /// counterclockwise from the upper left). They're an independent check on this model.
    #[test]
    fn olsak_hexagon_side_identities_hold() {
        let mut outlines = vec![Outline {
            a: (0, 2),
            b: (1, 3),
            c: (-1, 2),
        }];
        for side in 1..=4 {
            outlines.push(Outline::hexagon(side));
        }
        for a1 in 1..5 {
            for b1 in 1..5 {
                for c0 in -3..1 {
                    outlines.push(Outline {
                        a: (0, a1),
                        b: (0, b1),
                        c: (c0, c0 + 3),
                    });
                }
            }
        }

        for outline in outlines {
            if outline.cells().is_empty() {
                continue;
            }
            let c = Geometry::<Tri>::new(outline).clue_set_counts();
            let (a, b, cc, d, e, f) = (
                c.topleft as i64,
                c.bottomleft as i64,
                c.bottom as i64,
                c.bottomright as i64,
                c.topright as i64,
                c.top as i64,
            );
            assert_eq!(e, a + b - d, "E = A + B - D failed for {outline:?}");
            assert_eq!(f, cc + d - a, "F = C + D - A failed for {outline:?}");
        }
    }

    #[test]
    fn impossible_clue_counts_are_rejected() {
        assert!(
            Outline::from_clue_set_counts(ClueSetCounts {
                topleft: 99,
                bottomleft: 1,
                top: 1,
                topright: 1,
                bottom: 1,
                bottomright: 1,
            })
            .is_err()
        );
    }

    #[test]
    fn square_lanes_are_rows_then_columns() {
        let geo = Geometry::<Square>::new(Rect {
            width: 3,
            height: 2,
        });
        assert_eq!(geo.cell_count(), 6);
        assert_eq!(geo.family_count(), 2);

        let rows: Vec<Vec<u32>> = geo.family(0).map(|i| geo.lane(i).cells.clone()).collect();
        assert_eq!(rows, vec![vec![0, 1, 2], vec![3, 4, 5]]);

        let cols: Vec<Vec<u32>> = geo.family(1).map(|i| geo.lane(i).cells.clone()).collect();
        assert_eq!(cols, vec![vec![0, 3], vec![1, 4], vec![2, 5]]);
    }

    #[test]
    fn memberships_round_trip() {
        let square = Geometry::<Square>::new(Rect {
            width: 4,
            height: 3,
        });
        for cell in 0..square.cell_count() as u32 {
            assert_eq!(square.memberships(cell).len(), square.family_count());
            for m in square.memberships(cell) {
                assert_eq!(
                    square.lane(m.lane as usize).cells[m.position as usize],
                    cell
                );
            }
        }
        for outline in [doc_example(), Outline::hexagon(2)] {
            let geo = Geometry::<Tri>::new(outline);
            for cell in 0..geo.cell_count() as u32 {
                assert_eq!(geo.memberships(cell).len(), geo.family_count());
                for m in geo.memberships(cell) {
                    assert_eq!(geo.lane(m.lane as usize).cells[m.position as usize], cell);
                }
            }
        }
    }

    /// Unlike a square grid, two lanes can meet in two cells. The solver must not assume
    /// otherwise.
    #[test]
    fn two_lanes_can_share_two_cells() {
        let geo = Geometry::<Tri>::new(doc_example());
        let row_a: std::collections::HashSet<u32> = geo
            .lane(geo.family(0).start)
            .cells
            .iter()
            .copied()
            .collect();
        let slash: std::collections::HashSet<u32> = geo
            .lane(geo.family(1).start)
            .cells
            .iter()
            .copied()
            .collect();
        // Row `ABCDE` and `/` line `BAGFL` share both A and B.
        assert_eq!(row_a.intersection(&slash).count(), 2);
    }
}

// ===========================================================================================
// Typed grids
// ===========================================================================================
//
// `LaneMap` above is all the solver needs. Everything below is about *coordinates* — which the
// solver never uses, but the editor lives on. A `GridKind` marker fixes the coordinate type, so
// a square puzzle is indexed by `(x, y)` and a triddler by a `TriCoord`, checked at compile time.

use crate::layout::{CellShape, Guide, GutterLane, Point, TRI_HALF_BASE, TRI_ROW_HEIGHT, Vec2};

/// The dimensions of a square puzzle. (`Outline` plays the same role for triangular ones.)
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, Serialize, Deserialize)]
pub struct Rect {
    pub width: usize,
    pub height: usize,
}

/// The four sides of a square puzzle, matching `Side`'s role for triangular ones.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SquareSide {
    Top,
    Right,
    Bottom,
    Left,
}

/// Per-row tables that make `TriCoord` -> cell index O(1).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TriLookup {
    /// `rows + 1` entries; row `r` owns cells `row_start[r]..row_start[r + 1]`.
    row_start: Vec<u32>,
    /// The half-unit position of each row's leftmost cell.
    row_p_min: Vec<i32>,
    /// The extremes across all rows, so drawing can start at x = 0.
    p_min: i32,
    p_max: i32,
}

/// A marker type fixing a puzzle's coordinate system. Implemented by [`Square`] and [`Tri`].
pub trait GridKind: Copy + Clone + Eq + std::hash::Hash + std::fmt::Debug + 'static {
    /// How a cell is named: `(x, y)` for square, `TriCoord` for triangular.
    type Coord: Copy + Eq + Ord + std::hash::Hash + std::fmt::Debug;
    /// The shape's extent: `Rect` or `Outline`.
    type Dims: Clone + Copy + Eq + std::hash::Hash + std::fmt::Debug;
    /// A side that can be grown or shrunk: 4 of them, or 6.
    type Side: Copy + Eq + std::fmt::Debug + 'static;
    /// Precomputed tables for O(1) coordinate lookup.
    type Lookup: Clone + Eq + std::fmt::Debug;

    const FAMILY_COUNT: usize;
    const SIDES: &'static [Self::Side];

    /// Shapes are equal up to position, so equality and serialization compare normalized dims.
    fn normalize(dims: &Self::Dims) -> Self::Dims;

    fn build(dims: &Self::Dims) -> (LaneMap, Vec<Self::Coord>, Self::Lookup);

    /// The kind-erased form, for serialization and for runtime dispatch.
    fn to_shape(dims: &Self::Dims) -> Shape;
    /// The single place a runtime shape is narrowed back to a static kind.
    fn of_shape(shape: &Shape) -> Option<Self::Dims>;

    fn resized(dims: &Self::Dims, side: Self::Side, delta: i32) -> Option<Self::Dims>;

    /// `None` when the coordinate is outside the puzzle.
    fn cell_of(dims: &Self::Dims, lookup: &Self::Lookup, coord: Self::Coord) -> Option<u32>;

    /// Cells sharing an edge: 4 for a square, 3 for a triangle. Unused slots are `None`. These
    /// may be outside the puzzle; callers filter with `cell_of`.
    fn edge_neighbors(coord: Self::Coord) -> [Option<Self::Coord>; 4];

    /// One step along `family`, in the direction that family's lanes are stored.
    fn step(coord: Self::Coord, family: usize, forward: bool) -> Self::Coord;

    fn cell_shape(coord: Self::Coord) -> CellShape;
    /// The top-left of the cell's bounding box, in abstract units.
    fn cell_origin(dims: &Self::Dims, lookup: &Self::Lookup, coord: Self::Coord) -> Point;
    /// The whole puzzle's bounding box, in abstract units.
    fn extent(dims: &Self::Dims, lookup: &Self::Lookup) -> Vec2;
    /// Which cell contains this point, if any. O(1).
    fn cell_at(dims: &Self::Dims, lookup: &Self::Lookup, p: Point) -> Option<Self::Coord>;

    /// Unit vectors toward each neighbouring lane direction — 4 for a square, 6 for a triangle.
    /// Used to lay out the solver's contiguous-run widget.
    fn arm_directions() -> &'static [Vec2];

    /// Whether a family's clues are labelled at the *last* cell of its lanes rather than the
    /// first. True only for `\` lines, which are stored reading top-left to bottom-right but are
    /// labelled at the bottom — the same asymmetry both file formats have.
    fn clue_end_is_last(family: usize) -> bool;

    /// Erase the kind, for results that cross back to the GUI and the CLI.
    fn wrap_solution(s: crate::puzzle::Solution<Self>) -> crate::puzzle::DynSolution;
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Square;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Tri;

impl GridKind for Square {
    type Coord = (usize, usize);
    type Dims = Rect;
    type Side = SquareSide;
    type Lookup = ();

    const FAMILY_COUNT: usize = 2;
    const SIDES: &'static [SquareSide] = &[
        SquareSide::Top,
        SquareSide::Right,
        SquareSide::Bottom,
        SquareSide::Left,
    ];

    fn normalize(dims: &Rect) -> Rect {
        *dims
    }

    fn build(dims: &Rect) -> (LaneMap, Vec<(usize, usize)>, ()) {
        let (width, height) = (dims.width, dims.height);
        let mut lanes = vec![];

        // Cell `(x, y)` is index `y * width + x`, matching the old row-major `Array2` layout.
        for y in 0..height {
            lanes.push(Lane {
                family: 0,
                cells: (0..width).map(|x| (y * width + x) as u32).collect(),
                clue_set: None,
            });
        }
        let family_starts = vec![0, height, height + width];
        for x in 0..width {
            lanes.push(Lane {
                family: 1,
                cells: (0..height).map(|y| (y * width + x) as u32).collect(),
                clue_set: None,
            });
        }

        let coords = (0..height)
            .flat_map(|y| (0..width).map(move |x| (x, y)))
            .collect();
        (
            LaneMap::new(width * height, lanes, family_starts),
            coords,
            (),
        )
    }

    fn to_shape(dims: &Rect) -> Shape {
        Shape::Square {
            width: dims.width,
            height: dims.height,
        }
    }

    fn of_shape(shape: &Shape) -> Option<Rect> {
        match shape {
            Shape::Square { width, height } => Some(Rect {
                width: *width,
                height: *height,
            }),
            Shape::Triangular(_) => None,
        }
    }

    fn resized(dims: &Rect, side: SquareSide, delta: i32) -> Option<Rect> {
        let grow = |n: usize| -> Option<usize> {
            let n = n as i32 + delta;
            (n > 0).then_some(n as usize)
        };
        Some(match side {
            SquareSide::Top | SquareSide::Bottom => Rect {
                height: grow(dims.height)?,
                ..*dims
            },
            SquareSide::Left | SquareSide::Right => Rect {
                width: grow(dims.width)?,
                ..*dims
            },
        })
    }

    fn cell_of(dims: &Rect, _: &(), (x, y): (usize, usize)) -> Option<u32> {
        (x < dims.width && y < dims.height).then(|| (y * dims.width + x) as u32)
    }

    fn edge_neighbors((x, y): (usize, usize)) -> [Option<(usize, usize)>; 4] {
        [
            x.checked_sub(1).map(|x| (x, y)),
            Some((x + 1, y)),
            y.checked_sub(1).map(|y| (x, y)),
            Some((x, y + 1)),
        ]
    }

    fn step((x, y): (usize, usize), family: usize, forward: bool) -> (usize, usize) {
        let bump = |n: usize| if forward { n + 1 } else { n.wrapping_sub(1) };
        match family {
            0 => (bump(x), y),
            _ => (x, bump(y)),
        }
    }

    fn cell_shape(_: (usize, usize)) -> CellShape {
        CellShape::Square
    }

    fn cell_origin(_: &Rect, _: &(), (x, y): (usize, usize)) -> Point {
        Point::new(x as f32, y as f32)
    }

    fn extent(dims: &Rect, _: &()) -> Vec2 {
        Vec2::new(dims.width as f32, dims.height as f32)
    }

    fn cell_at(dims: &Rect, _: &(), p: Point) -> Option<(usize, usize)> {
        if p.x < 0.0 || p.y < 0.0 {
            return None;
        }
        let (x, y) = (p.x as usize, p.y as usize);
        (x < dims.width && y < dims.height).then_some((x, y))
    }

    fn arm_directions() -> &'static [Vec2] {
        const DIRS: [Vec2; 4] = [
            Vec2 { x: -1.0, y: 0.0 },
            Vec2 { x: 1.0, y: 0.0 },
            Vec2 { x: 0.0, y: -1.0 },
            Vec2 { x: 0.0, y: 1.0 },
        ];
        &DIRS
    }

    fn clue_end_is_last(_family: usize) -> bool {
        false
    }

    fn wrap_solution(s: crate::puzzle::Solution<Square>) -> crate::puzzle::DynSolution {
        crate::puzzle::DynSolution::Square(s)
    }
}

impl GridKind for Tri {
    type Coord = TriCoord;
    type Dims = Outline;
    type Side = Side;
    type Lookup = TriLookup;

    const FAMILY_COUNT: usize = 3;
    const SIDES: &'static [Side] = &[
        Side::Top,
        Side::UpperRight,
        Side::LowerRight,
        Side::Bottom,
        Side::LowerLeft,
        Side::UpperLeft,
    ];

    fn normalize(dims: &Outline) -> Outline {
        dims.normalized()
    }

    fn build(outline: &Outline) -> (LaneMap, Vec<TriCoord>, TriLookup) {
        let outline = *outline;
        let coords = outline.cells();
        let index_of: std::collections::HashMap<TriCoord, u32> = coords
            .iter()
            .enumerate()
            .map(|(i, t)| (*t, i as u32))
            .collect();

        // For each family: the coordinate to group by, the direction to walk from the *labelled*
        // end (the one that decides the clue set), the two clue-sets (the first wins on a corner
        // where both bounds bind), and whether the clues are actually read from the other end.
        //
        // That last flag exists only for `\` lines. Rows and `/` lines put clue index 0 at the
        // labelled end, as you'd expect; `\` lines do not — they read top-left to bottom-right,
        // *towards* the `bottom`/`bottomright` labels rather than away from them. This is not
        // documented; it was determined empirically, by finding the only one of the eight
        // possible direction combinations under which the worked example in `webpbn_tridder.md`
        // is consistent (and it solves that example completely). See HEXAGONAL_TODO.md.
        let families: [(
            fn(&TriCoord) -> i32,
            Direction,
            Direction,
            ClueSet,
            ClueSet,
            bool,
        ); 3] = [
            // Rows: read left-to-right, starting from the leftmost cell.
            (
                |t| t.a,
                Direction::Right,
                Direction::Left,
                ClueSet::TopLeft,
                ClueSet::BottomLeft,
                false,
            ),
            // `/` lines: read downward-left, starting from the topmost cell.
            (
                |t| t.b,
                Direction::DownLeft,
                Direction::UpRight,
                ClueSet::Top,
                ClueSet::TopRight,
                false,
            ),
            // `\` lines: labelled at the bottom, but read from the far end, downward-right.
            (
                |t| t.c,
                Direction::UpLeft,
                Direction::DownRight,
                ClueSet::Bottom,
                ClueSet::BottomRight,
                true,
            ),
        ];

        let mut lanes = vec![];
        let mut family_starts = vec![0];

        for (family, (key, forward, backward, near_set, far_set, read_from_far_end)) in
            families.into_iter().enumerate()
        {
            let (lo, hi) = match family {
                0 => outline.a,
                1 => outline.b,
                _ => outline.c,
            };
            for k in lo..=hi {
                let Some(any) = coords.iter().find(|t| key(t) == k) else {
                    continue; // A bound can be slack; that lane simply doesn't exist.
                };

                // Walk backward to the end the clues are written from...
                let mut start = *any;
                while outline.contains(start.step(backward)) {
                    start = start.step(backward);
                }

                // ...then forward, collecting the whole lane.
                let mut cells = vec![index_of[&start]];
                let mut at = start;
                while outline.contains(at.step(forward)) {
                    at = at.step(forward);
                    cells.push(index_of[&at]);
                }
                if read_from_far_end {
                    cells.reverse();
                }

                // Which bound stopped us at `start`? That names the clue set.
                let past = start.step(backward);
                let near = match family {
                    0 => past.b < outline.b.0,
                    1 => past.a < outline.a.0,
                    _ => past.a > outline.a.1,
                };

                lanes.push(Lane {
                    family,
                    cells,
                    clue_set: Some(if near { near_set } else { far_set }),
                });
            }
            family_starts.push(lanes.len());
        }

        // Rows are contiguous runs of the dense numbering, so a start offset and the leftmost
        // half-unit per row are enough to place any coordinate in O(1).
        let mut row_start = vec![0u32];
        let mut row_p_min = vec![];
        let (mut p_min, mut p_max) = (i32::MAX, i32::MIN);
        for a in outline.a.0..=outline.a.1 {
            let mut first = None;
            let mut count = 0u32;
            for t in coords.iter().filter(|t| t.a == a) {
                let (_, p) = t.to_row_and_half_unit();
                first.get_or_insert(p);
                p_min = p_min.min(p);
                p_max = p_max.max(p);
                count += 1;
            }
            row_p_min.push(first.unwrap_or(0));
            row_start.push(row_start.last().unwrap() + count);
        }

        let lookup = TriLookup {
            row_start,
            row_p_min,
            p_min: if p_min == i32::MAX { 0 } else { p_min },
            p_max: if p_max == i32::MIN { 0 } else { p_max },
        };

        (
            LaneMap::new(coords.len(), lanes, family_starts),
            coords,
            lookup,
        )
    }

    fn to_shape(dims: &Outline) -> Shape {
        Shape::Triangular(*dims)
    }

    fn of_shape(shape: &Shape) -> Option<Outline> {
        match shape {
            Shape::Triangular(o) => Some(*o),
            Shape::Square { .. } => None,
        }
    }

    fn resized(dims: &Outline, side: Side, delta: i32) -> Option<Outline> {
        dims.resized(side, delta)
    }

    fn cell_of(outline: &Outline, lookup: &TriLookup, t: TriCoord) -> Option<u32> {
        if !t.is_valid() {
            return None;
        }
        let row = usize::try_from(t.a - outline.a.0).ok()?;
        let start = *lookup.row_start.get(row)?;
        let len = lookup.row_start.get(row + 1)? - start;
        let (_, p) = t.to_row_and_half_unit();
        // A row of a convex outline is contiguous, so this range check *is* the containment
        // check — no scan needed.
        let offset = u32::try_from(p - lookup.row_p_min[row]).ok()?;
        (offset < len).then_some(start + offset)
    }

    fn edge_neighbors(t: TriCoord) -> [Option<TriCoord>; 4] {
        let TriCoord { a, b, c } = t;
        if t.points_up() {
            [
                Some(TriCoord::new(a, b - 1, c)), // left
                Some(TriCoord::new(a, b, c + 1)), // right
                Some(TriCoord::new(a + 1, b, c)), // below, across the base
                None,
            ]
        } else {
            [
                Some(TriCoord::new(a, b, c - 1)), // left
                Some(TriCoord::new(a, b + 1, c)), // right
                Some(TriCoord::new(a - 1, b, c)), // above, across the top edge
                None,
            ]
        }
    }

    fn step(t: TriCoord, family: usize, forward: bool) -> TriCoord {
        // The direction each family's lanes are stored in; see `Tri::build`.
        let dir = match (family, forward) {
            (0, true) => Direction::Right,
            (0, false) => Direction::Left,
            (1, true) => Direction::DownLeft,
            (1, false) => Direction::UpRight,
            (_, true) => Direction::DownRight,
            (_, false) => Direction::UpLeft,
        };
        t.step(dir)
    }

    fn cell_shape(t: TriCoord) -> CellShape {
        if t.points_up() {
            CellShape::UpTriangle
        } else {
            CellShape::DownTriangle
        }
    }

    fn cell_origin(outline: &Outline, lookup: &TriLookup, t: TriCoord) -> Point {
        let (a, p) = t.to_row_and_half_unit();
        Point::new(
            (p - lookup.p_min) as f32 * TRI_HALF_BASE,
            (a - outline.a.0) as f32 * TRI_ROW_HEIGHT,
        )
    }

    fn extent(outline: &Outline, lookup: &TriLookup) -> Vec2 {
        let rows = (outline.a.1 - outline.a.0 + 1) as f32;
        // The rightmost cell starts at `p_max` and spans two half-units.
        Vec2::new(
            (lookup.p_max - lookup.p_min + 2) as f32 * TRI_HALF_BASE,
            rows * TRI_ROW_HEIGHT,
        )
    }

    fn cell_at(outline: &Outline, lookup: &TriLookup, point: Point) -> Option<TriCoord> {
        if point.x < 0.0 || point.y < 0.0 {
            return None;
        }
        let py = point.y / TRI_ROW_HEIGHT;
        let px = point.x / TRI_HALF_BASE;
        let (row, column) = (py as i32, px as i32);
        let (fy, fx) = (py - row as f32, px - column as f32);

        let a = outline.a.0 + row;
        // Two cells overlap this half-unit strip: the one starting here and the one starting one
        // half-unit earlier. They are separated by a single straight diagonal, so one comparison
        // picks between them — which one depends on whether the near cell points up.
        let near_p = lookup.p_min + column;
        let near_points_up = (near_p + a).rem_euclid(2) == 0;
        let take_near = if near_points_up {
            // Its apex is at the strip's right edge; it owns everything below-right of the
            // diagonal from bottom-left to top-right.
            fx + fy > 1.0
        } else {
            // Its apex is at the strip's bottom-right; it owns everything right of the diagonal
            // from top-left to bottom-right.
            fx > fy
        };
        let p = if take_near { near_p } else { near_p - 1 };

        let coord = TriCoord::from_row_and_half_unit(a, p);
        Tri::cell_of(outline, lookup, coord).map(|_| coord)
    }

    fn arm_directions() -> &'static [Vec2] {
        // 0 degrees, plus or minus 60, and their opposites: the six directions a triangular lane
        // can leave a cell in.
        const S: f32 = 0.866_025_4; // sin 60
        const DIRS: [Vec2; 6] = [
            Vec2 { x: -1.0, y: 0.0 },
            Vec2 { x: 1.0, y: 0.0 },
            Vec2 { x: -0.5, y: -S },
            Vec2 { x: 0.5, y: S },
            Vec2 { x: 0.5, y: -S },
            Vec2 { x: -0.5, y: S },
        ];
        &DIRS
    }

    fn clue_end_is_last(family: usize) -> bool {
        family == 2
    }

    fn wrap_solution(s: crate::puzzle::Solution<Tri>) -> crate::puzzle::DynSolution {
        crate::puzzle::DynSolution::Tri(s)
    }
}

/// A puzzle's shape, with coordinates. Wraps a [`LaneMap`] (what the solver uses) and adds
/// everything positional: the coordinate for each cell, O(1) lookup in both directions, and the
/// abstract layout the editor draws from.
///
/// Position is not part of a shape's identity — sliding a triangular outline sideways describes
/// the same puzzle — so equality and hashing compare *normalized* dimensions. The dimensions
/// themselves are stored as given, which is what lets a resize leave existing coordinates alone.
#[derive(Clone, Debug)]
pub struct Geometry<K: GridKind> {
    dims: K::Dims,
    lookup: K::Lookup,
    /// Parallel to cell indices.
    coords: Vec<K::Coord>,
    guides: Vec<Guide>,
    gutters: Vec<(Option<ClueSet>, Vec<GutterLane>)>,
    lanes: LaneMap,
}

impl<K: GridKind> PartialEq for Geometry<K> {
    fn eq(&self, other: &Self) -> bool {
        K::normalize(&self.dims) == K::normalize(&other.dims)
    }
}
impl<K: GridKind> Eq for Geometry<K> {}

impl<K: GridKind> std::hash::Hash for Geometry<K> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        K::normalize(&self.dims).hash(state);
    }
}

impl<K: GridKind> Geometry<K> {
    pub fn new(dims: K::Dims) -> Geometry<K> {
        let (lanes, coords, lookup) = K::build(&dims);
        let guides = build_guides::<K>(&dims, &lookup, &coords, &lanes);
        let gutters = build_gutters::<K>(&dims, &lookup, &coords, &lanes);
        Geometry {
            dims,
            lookup,
            coords,
            guides,
            gutters,
            lanes,
        }
    }

    pub fn dims(&self) -> &K::Dims {
        &self.dims
    }

    pub fn shape(&self) -> Shape {
        K::to_shape(&self.dims)
    }

    /// The lane structure — all the solver needs.
    pub fn lane_map(&self) -> &LaneMap {
        &self.lanes
    }

    pub fn cell_count(&self) -> usize {
        self.lanes.cell_count()
    }

    // Delegated to `LaneMap`, so callers holding a `Geometry` don't have to reach through.
    pub fn family_count(&self) -> usize {
        self.lanes.family_count()
    }

    pub fn family(&self, family: usize) -> std::ops::Range<usize> {
        self.lanes.family(family)
    }

    pub fn lane(&self, idx: usize) -> &Lane {
        self.lanes.lane(idx)
    }

    pub fn lane_count(&self) -> usize {
        self.lanes.lane_count()
    }

    pub fn memberships(&self, cell: u32) -> &[Membership] {
        self.lanes.memberships(cell)
    }

    pub fn coord(&self, cell: u32) -> K::Coord {
        self.coords[cell as usize]
    }

    pub fn coords(&self) -> &[K::Coord] {
        &self.coords
    }

    pub fn cell(&self, coord: K::Coord) -> Option<u32> {
        K::cell_of(&self.dims, &self.lookup, coord)
    }

    /// Cells sharing an edge with this one, already filtered to those inside the puzzle.
    pub fn neighbor_cells(&self, cell: u32) -> impl Iterator<Item = u32> + '_ {
        K::edge_neighbors(self.coord(cell))
            .into_iter()
            .flatten()
            .filter_map(|c| self.cell(c))
    }

    /// How far a run of matching cells extends from `cell` along each family, as
    /// `(backward, forward)` counts not including `cell` itself.
    pub fn runs(&self, cell: u32, matches: impl Fn(u32) -> bool) -> Vec<(usize, usize)> {
        self.lanes
            .memberships(cell)
            .iter()
            .map(|m| {
                let lane = &self.lanes.lane(m.lane as usize).cells;
                let pos = m.position as usize;
                let back = (0..pos).rev().take_while(|i| matches(lane[*i])).count();
                let fwd = (pos + 1..lane.len())
                    .take_while(|i| matches(lane[*i]))
                    .count();
                (back, fwd)
            })
            .collect()
    }

    pub fn extent(&self) -> Vec2 {
        K::extent(&self.dims, &self.lookup)
    }

    pub fn cell_shape(&self, cell: u32) -> CellShape {
        K::cell_shape(self.coord(cell))
    }

    pub fn cell_origin(&self, cell: u32) -> Point {
        K::cell_origin(&self.dims, &self.lookup, self.coord(cell))
    }

    /// Which cell contains this point, in abstract units. O(1).
    pub fn cell_at(&self, p: Point) -> Option<K::Coord> {
        K::cell_at(&self.dims, &self.lookup, p)
    }

    /// Lane indices in the given clue set, in the order webpbn lists them.
    pub fn lanes_in_clue_set(&self, clue_set: ClueSet) -> Vec<usize> {
        self.lanes.lanes_in_clue_set(clue_set)
    }

    pub fn clue_set_counts(&self) -> ClueSetCounts {
        self.lanes.clue_set_counts()
    }

    /// Unit vectors toward each neighbouring lane direction: 4 for a square, 6 for a triangle,
    /// listed as `(backward, forward)` pairs per family to match `runs`.
    pub fn arm_directions(&self) -> &'static [Vec2] {
        K::arm_directions()
    }

    /// Boundary lines between lanes, for drawing the grid.
    pub fn guides(&self) -> &[Guide] {
        &self.guides
    }

    /// Where each lane's clues go: two gutters for a square puzzle, six for a triddler.
    pub fn gutters(&self) -> &[(Option<ClueSet>, Vec<GutterLane>)] {
        &self.gutters
    }

    /// A single row with nothing crossing it, so line logic can be tested without a second
    /// family over-constraining the puzzle. Deliberately has fewer families than `Square`
    /// normally would; only the solver ever looks at it.
    #[cfg(test)]
    pub fn single_lane(len: usize) -> Geometry<Square> {
        let lanes = vec![Lane {
            family: 0,
            cells: (0..len as u32).collect(),
            clue_set: None,
        }];
        Geometry {
            dims: Rect {
                width: len,
                height: 1,
            },
            lookup: (),
            coords: (0..len).map(|x| (x, 0)).collect(),
            guides: vec![],
            gutters: vec![],
            lanes: LaneMap::new(len, lanes, vec![0, 1]),
        }
    }

    pub fn resized(&self, side: K::Side, delta: i32) -> Option<Geometry<K>> {
        K::resized(&self.dims, side, delta).map(Geometry::new)
    }

    /// Iterate the puzzle row by row, carrying the layout each cell needs to be drawn.
    ///
    /// Both levels are cheap after monomorphization: the inner one walks a slice and advances a
    /// single `f32`, with no coordinate arithmetic and no lookups.
    pub fn rows(&self) -> RowIter<'_, K> {
        RowIter {
            geometry: self,
            family: self.lanes.family(0),
        }
    }
}

pub struct RowIter<'a, K: GridKind> {
    geometry: &'a Geometry<K>,
    family: std::ops::Range<usize>,
}

impl<'a, K: GridKind> Iterator for RowIter<'a, K> {
    type Item = RowDraw<'a, K>;

    fn next(&mut self) -> Option<RowDraw<'a, K>> {
        let lane = self.family.next()?;
        let cells = &self.geometry.lanes.lane(lane).cells;
        Some(RowDraw {
            lane,
            row: lane - self.geometry.lanes.family(0).start,
            origin: cells
                .first()
                .map(|c| self.geometry.cell_origin(*c))
                .unwrap_or_default(),
            geometry: self.geometry,
            cells,
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.family.len(), Some(self.family.len()))
    }
}

impl<'a, K: GridKind> ExactSizeIterator for RowIter<'a, K> {}

pub struct RowDraw<'a, K: GridKind> {
    /// Index into `LaneMap::lanes()`, so also into `Puzzle::lines`.
    pub lane: usize,
    /// The row's position within its family, for the every-fifth-line emphasis.
    pub row: usize,
    /// The top-left of the row's bounding box.
    pub origin: Point,
    geometry: &'a Geometry<K>,
    cells: &'a [u32],
}

impl<'a, K: GridKind> RowDraw<'a, K> {
    pub fn len(&self) -> usize {
        self.cells.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    pub fn cells(&self) -> CellIter<'a, K> {
        let first = self.cells.first().copied();
        CellIter {
            cells: self.cells,
            at: 0,
            origin: self.origin,
            step: match first.map(|c| self.geometry.cell_shape(c)) {
                // Triangles overlap by half their width; squares simply abut.
                Some(CellShape::UpTriangle) | Some(CellShape::DownTriangle) => TRI_HALF_BASE,
                _ => 1.0,
            },
            shape: first
                .map(|c| self.geometry.cell_shape(c))
                .unwrap_or(CellShape::Square),
            geometry: self.geometry,
        }
    }
}

pub struct CellIter<'a, K: GridKind> {
    cells: &'a [u32],
    at: usize,
    origin: Point,
    step: f32,
    shape: CellShape,
    geometry: &'a Geometry<K>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct CellDraw<K: GridKind> {
    /// Dense index — use it directly on `Solution::cells`.
    pub cell: u32,
    pub coord: K::Coord,
    pub shape: CellShape,
    /// The top-left of the cell's bounding box.
    pub origin: Point,
}

impl<'a, K: GridKind> Iterator for CellIter<'a, K> {
    type Item = CellDraw<K>;

    fn next(&mut self) -> Option<CellDraw<K>> {
        let cell = *self.cells.get(self.at)?;
        let origin = self.origin;
        let shape = self.shape;

        self.at += 1;
        self.origin.x += self.step;
        self.shape = match shape {
            CellShape::Square => CellShape::Square,
            CellShape::UpTriangle => CellShape::DownTriangle,
            CellShape::DownTriangle => CellShape::UpTriangle,
        };

        Some(CellDraw {
            cell,
            coord: self.geometry.coords[cell as usize],
            shape,
            origin,
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.cells.len() - self.at;
        (n, Some(n))
    }
}

impl<'a, K: GridKind> ExactSizeIterator for CellIter<'a, K> {}

/// One boundary line per lane, plus a closing one per family.
///
/// A lane's outer edge is the union of its cells' bounding boxes projected onto the lane's
/// direction, which for both kinds is just the segment from the first cell's leading corner to
/// the last cell's trailing corner.
fn build_guides<K: GridKind>(
    dims: &K::Dims,
    lookup: &K::Lookup,
    coords: &[K::Coord],
    lanes: &LaneMap,
) -> Vec<Guide> {
    let mut res = vec![];
    for family in 0..lanes.family_count() {
        for (index, lane) in lanes.family(family).enumerate() {
            let cells = &lanes.lane(lane).cells;
            let (Some(&first), Some(&last)) = (cells.first(), cells.last()) else {
                continue;
            };
            let origin = |c: u32| K::cell_origin(dims, lookup, coords[c as usize]);
            let shape = |c: u32| K::cell_shape(coords[c as usize]);

            // The lane's own direction, from its first cell's centre to its last. Used (rather
            // than a fixed vertex index) so the guide runs parallel to the lane instead of
            // connecting whichever corners happen to share an index — which for a square cell
            // means "top-left of the first cell" to "bottom-left of the last", a diagonal that
            // reads as a slight tilt across the whole grid.
            let first_center = shape(first).center(origin(first));
            let last_center = shape(last).center(origin(last));
            let mut d = Vec2::new(
                last_center.x - first_center.x,
                last_center.y - first_center.y,
            );
            let len = (d.x * d.x + d.y * d.y).sqrt();
            d = if len > 1e-6 {
                Vec2::new(d.x / len, d.y / len)
            } else {
                Vec2::new(1.0, 0.0) // A single-cell lane has no real direction.
            };
            // Perpendicular to `d`: picks a consistent side of the lane to draw the guide on.
            let n = Vec2::new(-d.y, d.x);

            // The vertex on the `n`-side of the cell, breaking ties by `d` so the first cell
            // contributes its backward corner and the last cell its forward one — together
            // spanning the lane's full length rather than just one cell's width.
            let edge_vertex = |cell: u32, forward: bool| -> Point {
                let (points, count) = shape(cell).vertices(origin(cell));
                let mut best = points[0];
                let mut best_key = (f32::MAX, f32::MAX);
                for p in &points[..count] {
                    let n_dot = p.x * n.x + p.y * n.y;
                    let d_dot = p.x * d.x + p.y * d.y;
                    let key = (n_dot, if forward { -d_dot } else { d_dot });
                    if key < best_key {
                        best_key = key;
                        best = *p;
                    }
                }
                best
            };

            res.push(Guide {
                from: edge_vertex(first, false),
                to: edge_vertex(last, true),
                family,
                index,
                emphasis: index % 5 == 0,
            });
        }
    }
    res
}

/// Where each lane's clues are drawn.
///
/// The gutter sits at the end the file formats label, which spreads a hexagon's clues over all six
/// of its sides. For rows and `/` lines that is the end the lane is read from; for `\` lines it is
/// the other end, so their clues are drawn in the reverse of the stored order — the convention
/// Olsak describes as reading "from underneath upstairs".
fn build_gutters<K: GridKind>(
    dims: &K::Dims,
    lookup: &K::Lookup,
    coords: &[K::Coord],
    lanes: &LaneMap,
) -> Vec<(Option<ClueSet>, Vec<GutterLane>)> {
    let mut res: Vec<(Option<ClueSet>, Vec<GutterLane>)> = vec![];
    for family in 0..lanes.family_count() {
        let at_last = K::clue_end_is_last(family);
        for lane in lanes.family(family) {
            let cells = &lanes.lane(lane).cells;
            let Some(end) = (if at_last { cells.last() } else { cells.first() }) else {
                continue;
            };
            let clue_set = lanes.lane(lane).clue_set;

            let coord = coords[*end as usize];
            let origin = K::cell_origin(dims, lookup, coord);
            let center = K::cell_shape(coord).center(origin);
            // Step off the end of the lane; that direction leads away from the grid.
            //
            // Two steps, not one: consecutive cells of a triangular lane alternate ▲/▼, and the
            // two displacements differ. Only their sum is parallel to the lane itself.
            let outward = {
                let past = K::step(K::step(coord, family, at_last), family, at_last);
                let far = K::cell_origin(dims, lookup, past);
                let d = Vec2::new(far.x - origin.x, far.y - origin.y);
                let len = (d.x * d.x + d.y * d.y).sqrt();
                if len > 0.0 {
                    Vec2::new(d.x / len, d.y / len)
                } else {
                    Vec2::new(-1.0, 0.0)
                }
            };
            // The midpoint of the edge the lane exits through — *not* the centroid pushed
            // outward. A ▲ and a ▼ have centroids at different heights, so using those would
            // bunch adjacent rows' clues together; their exit edges are properly spaced.
            let anchor = {
                let shape = K::cell_shape(coord);
                let (points, n) = shape.vertices(origin);
                let mut best = center;
                let mut best_dot = f32::MIN;
                for i in 0..n {
                    let (a, b) = (points[i], points[(i + 1) % n]);
                    let mid = Point::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0);
                    let dot = (mid.x - center.x) * outward.x + (mid.y - center.y) * outward.y;
                    if dot > best_dot {
                        best_dot = dot;
                        best = mid;
                    }
                }
                best
            };

            let entry = res.iter_mut().find(|(cs, _)| *cs == clue_set);
            let bucket = match entry {
                Some(e) => &mut e.1,
                None => {
                    res.push((clue_set, vec![]));
                    &mut res.last_mut().unwrap().1
                }
            };
            bucket.push(GutterLane {
                lane,
                anchor,
                outward,
                reversed: at_last,
            });
        }
    }
    res
}

#[cfg(test)]
mod typed_tests {
    use super::*;

    fn tri_shapes() -> Vec<Outline> {
        let mut v = vec![
            // The webpbn worked example: an off-centre bend.
            Outline {
                a: (0, 2),
                b: (1, 3),
                c: (-1, 2),
            },
            // The same shape as the one above, slid by (a, b) += 5 then (b, c) += 4, to check
            // position is carried through faithfully rather than normalized away.
            Outline {
                a: (5, 7),
                b: (10, 12),
                c: (3, 6),
            },
        ];
        for side in 1..=4 {
            v.push(Outline::hexagon(side));
        }
        v
    }

    fn square_shapes() -> Vec<Rect> {
        vec![
            Rect {
                width: 1,
                height: 1,
            },
            Rect {
                width: 5,
                height: 3,
            },
            Rect {
                width: 3,
                height: 7,
            },
        ]
    }

    #[test]
    fn coord_and_cell_round_trip() {
        for dims in square_shapes() {
            let geo = Geometry::<Square>::new(dims);
            for cell in 0..geo.cell_count() as u32 {
                assert_eq!(geo.cell(geo.coord(cell)), Some(cell));
            }
            assert_eq!(geo.cell((dims.width, 0)), None);
            assert_eq!(geo.cell((0, dims.height)), None);
        }
        for dims in tri_shapes() {
            let geo = Geometry::<Tri>::new(dims);
            for cell in 0..geo.cell_count() as u32 {
                assert_eq!(geo.cell(geo.coord(cell)), Some(cell));
            }
        }
    }

    /// Anything outside the outline must be rejected, not aliased onto some other cell.
    #[test]
    fn coords_outside_the_outline_have_no_cell() {
        for dims in tri_shapes() {
            let geo = Geometry::<Tri>::new(dims);
            let inside: std::collections::HashSet<TriCoord> =
                geo.coords().iter().copied().collect();
            for a in dims.a.0 - 2..=dims.a.1 + 2 {
                for b in dims.b.0 - 2..=dims.b.1 + 2 {
                    for c in dims.c.0 - 2..=dims.c.1 + 2 {
                        let t = TriCoord::new(a, b, c);
                        if !t.is_valid() {
                            continue;
                        }
                        assert_eq!(
                            geo.cell(t).is_some(),
                            inside.contains(&t),
                            "{t:?} in {dims:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn half_unit_round_trips() {
        for a in -4..4 {
            for p in -6..6 {
                let t = TriCoord::from_row_and_half_unit(a, p);
                assert!(t.is_valid());
                assert_eq!(t.to_row_and_half_unit(), (a, p));
            }
        }
    }

    /// Every cell's centroid must hit that cell — the basic hit-test guarantee.
    #[test]
    fn hit_test_finds_each_cell_from_its_center() {
        for dims in square_shapes() {
            let geo = Geometry::<Square>::new(dims);
            for cell in 0..geo.cell_count() as u32 {
                let c = geo.cell_shape(cell).center(geo.cell_origin(cell));
                assert_eq!(geo.cell_at(c), Some(geo.coord(cell)), "{c:?} in {dims:?}");
            }
        }
        for dims in tri_shapes() {
            let geo = Geometry::<Tri>::new(dims);
            for cell in 0..geo.cell_count() as u32 {
                let c = geo.cell_shape(cell).center(geo.cell_origin(cell));
                assert_eq!(geo.cell_at(c), Some(geo.coord(cell)), "{c:?} in {dims:?}");
            }
        }
    }

    /// Sweep the whole bounding box: wherever the hit test claims a cell, the point really must
    /// be inside that cell's polygon. This is what catches an off-by-one in the diagonal test.
    #[test]
    fn hit_test_agrees_with_the_polygons() {
        let mut seed = 0x9E3779B9u32;
        let mut rand = move || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 8) as f32 / (1 << 24) as f32
        };

        for dims in tri_shapes() {
            let geo = Geometry::<Tri>::new(dims);
            let extent = geo.extent();
            let mut hits = 0;
            for _ in 0..4000 {
                let p = Point::new(rand() * extent.x, rand() * extent.y);
                let Some(coord) = geo.cell_at(p) else {
                    continue;
                };
                let cell = geo
                    .cell(coord)
                    .expect("hit test returned a coord with no cell");
                assert!(
                    geo.cell_shape(cell).contains(geo.cell_origin(cell), p),
                    "{p:?} was assigned {coord:?}, which does not contain it ({dims:?})"
                );
                hits += 1;
            }
            assert!(
                hits > 500,
                "only {hits} hits — the sweep is not exercising much"
            );
        }
    }

    #[test]
    fn rows_visit_every_cell_once_in_dense_order() {
        for dims in square_shapes() {
            let geo = Geometry::<Square>::new(dims);
            let seen: Vec<u32> = geo
                .rows()
                .flat_map(|r| r.cells().collect::<Vec<_>>())
                .map(|c| c.cell)
                .collect();
            assert_eq!(seen, (0..geo.cell_count() as u32).collect::<Vec<_>>());
        }
        for dims in tri_shapes() {
            let geo = Geometry::<Tri>::new(dims);
            let seen: Vec<u32> = geo
                .rows()
                .flat_map(|r| r.cells().collect::<Vec<_>>())
                .map(|c| c.cell)
                .collect();
            assert_eq!(seen, (0..geo.cell_count() as u32).collect::<Vec<_>>());
        }
    }

    /// The iterators' cheap incremental layout must match the authoritative per-coordinate one.
    #[test]
    fn iterator_layout_matches_direct_layout() {
        for dims in tri_shapes() {
            let geo = Geometry::<Tri>::new(dims);
            for row in geo.rows() {
                for drawn in row.cells() {
                    let direct = geo.cell_origin(drawn.cell);
                    assert!(
                        (drawn.origin.x - direct.x).abs() < 1e-4
                            && (drawn.origin.y - direct.y).abs() < 1e-4,
                        "{:?} vs {direct:?}",
                        drawn.origin
                    );
                    assert_eq!(drawn.shape, geo.cell_shape(drawn.cell));
                    assert_eq!(drawn.coord, geo.coord(drawn.cell));
                }
            }
        }
    }

    #[test]
    fn extent_covers_every_cell() {
        for dims in tri_shapes() {
            let geo = Geometry::<Tri>::new(dims);
            let extent = geo.extent();
            for cell in 0..geo.cell_count() as u32 {
                let o = geo.cell_origin(cell);
                let size = geo.cell_shape(cell).size();
                assert!(o.x >= -1e-4 && o.y >= -1e-4, "{o:?}");
                assert!(
                    o.x + size.x <= extent.x + 1e-4 && o.y + size.y <= extent.y + 1e-4,
                    "cell {cell} at {o:?} escapes {extent:?} of {dims:?}"
                );
            }
        }
    }

    #[test]
    fn edge_neighbors_are_symmetric() {
        for dims in tri_shapes() {
            let geo = Geometry::<Tri>::new(dims);
            for cell in 0..geo.cell_count() as u32 {
                for neighbor in geo.neighbor_cells(cell) {
                    assert!(
                        geo.neighbor_cells(neighbor).any(|back| back == cell),
                        "{cell} -> {neighbor} is not mutual in {dims:?}"
                    );
                }
                // A triangle has three edges; cells on the boundary have fewer neighbours.
                assert!(geo.neighbor_cells(cell).count() <= 3);
            }
        }
        for dims in square_shapes() {
            let geo = Geometry::<Square>::new(dims);
            for cell in 0..geo.cell_count() as u32 {
                for neighbor in geo.neighbor_cells(cell) {
                    assert!(geo.neighbor_cells(neighbor).any(|back| back == cell));
                }
                assert!(geo.neighbor_cells(cell).count() <= 4);
            }
        }
    }

    /// Position is not part of a shape's identity, but it *is* preserved in the coordinates.
    #[test]
    fn translated_outlines_are_equal_but_keep_their_coordinates() {
        let base = Outline {
            a: (0, 2),
            b: (1, 3),
            c: (-1, 2),
        };
        let moved = Outline {
            a: (5, 7),
            b: (10, 12),
            c: (3, 6),
        };
        let (g1, g2) = (Geometry::<Tri>::new(base), Geometry::<Tri>::new(moved));
        assert_eq!(g1, g2, "same shape, different position");
        assert_eq!(g1.cell_count(), g2.cell_count());
        assert_eq!(g1.coord(0).a, 0);
        assert_eq!(g2.coord(0).a, 5, "coordinates are stored as given");
    }

    /// The whole point of storing dims raw: growing a side must not renumber existing cells.
    #[test]
    fn resizing_leaves_existing_coordinates_alone() {
        for dims in tri_shapes() {
            let geo = Geometry::<Tri>::new(dims);
            for side in Tri::SIDES {
                let Some(bigger) = geo.resized(*side, 1) else {
                    continue;
                };
                for cell in 0..geo.cell_count() as u32 {
                    let coord = geo.coord(cell);
                    assert!(
                        bigger.cell(coord).is_some(),
                        "growing {side:?} lost {coord:?} from {dims:?}"
                    );
                }
            }
        }
    }

    /// A square grid's guides must be axis-aligned and span the full width/height, not a diagonal
    /// clipped to one cell — the bug that made the whole grid look slightly tilted.
    #[test]
    fn square_guides_are_not_skewed() {
        let geo = Geometry::<Square>::new(Rect {
            width: 6,
            height: 4,
        });
        for guide in geo.guides() {
            match guide.family {
                0 => {
                    assert!(
                        (guide.from.y - guide.to.y).abs() < 1e-4,
                        "row guide tilts: {guide:?}"
                    );
                    assert!(
                        (guide.to.x - guide.from.x - 6.0).abs() < 1e-4,
                        "row guide doesn't span the full width: {guide:?}"
                    );
                }
                _ => {
                    assert!(
                        (guide.from.x - guide.to.x).abs() < 1e-4,
                        "column guide tilts: {guide:?}"
                    );
                    assert!(
                        (guide.to.y - guide.from.y - 4.0).abs() < 1e-4,
                        "column guide doesn't span the full height: {guide:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn square_resizing_changes_one_dimension() {
        let geo = Geometry::<Square>::new(Rect {
            width: 4,
            height: 3,
        });
        assert_eq!(
            geo.resized(SquareSide::Right, 1).unwrap().dims(),
            &Rect {
                width: 5,
                height: 3
            }
        );
        assert_eq!(
            geo.resized(SquareSide::Top, -1).unwrap().dims(),
            &Rect {
                width: 4,
                height: 2
            }
        );
        assert!(
            Geometry::<Square>::new(Rect {
                width: 1,
                height: 1
            })
            .resized(SquareSide::Left, -1)
            .is_none()
        );
    }

    /// Clues are laid out along their own lane's direction, so there are three clue blocks (one
    /// per family), not six. The six clue *sets* are three families x two boundary edges: the two
    /// sets of a family share a direction but cover disjoint lanes, so they nest rather than
    /// overlap.
    #[test]
    fn hexagon_gutters_form_three_blocks() {
        let geo = Geometry::<Tri>::new(Outline::hexagon(3));
        let extent = geo.extent();
        let middle = Point::new(extent.x / 2.0, extent.y / 2.0);

        let mut directions = vec![];
        for (clue_set, lanes) in geo.gutters() {
            let set = clue_set.expect("a triddler labels every lane");
            let first = lanes[0];
            // Every lane in a clue set shares one outward direction.
            for l in lanes {
                assert!(
                    (l.outward.x - first.outward.x).abs() < 1e-4
                        && (l.outward.y - first.outward.y).abs() < 1e-4,
                    "{set:?} lanes disagree about which way is out"
                );
                // The anchor must be outside the hexagon's middle in that direction.
                let away =
                    (l.anchor.x - middle.x) * l.outward.x + (l.anchor.y - middle.y) * l.outward.y;
                assert!(away > 0.0, "{set:?} anchor {:?} faces inward", l.anchor);
            }
            directions.push((set, first.outward));
        }

        assert_eq!(directions.len(), 6, "a hexagon uses all six clue sets");

        // Exactly three distinct directions, one per family.
        let mut distinct: Vec<Vec2> = vec![];
        for (_, d) in &directions {
            if !distinct
                .iter()
                .any(|e: &Vec2| (e.x - d.x).abs() < 1e-4 && (e.y - d.y).abs() < 1e-4)
            {
                distinct.push(*d);
            }
        }
        assert_eq!(
            distinct.len(),
            3,
            "one clue direction per family: {directions:?}"
        );

        // The two sets of one family must cover disjoint lanes, or their clues would collide.
        for (a, b) in [
            (ClueSet::TopLeft, ClueSet::BottomLeft),
            (ClueSet::Top, ClueSet::TopRight),
            (ClueSet::Bottom, ClueSet::BottomRight),
        ] {
            let lanes_of = |set| -> std::collections::HashSet<usize> {
                geo.gutters()
                    .iter()
                    .filter(|(cs, _)| *cs == Some(set))
                    .flat_map(|(_, l)| l.iter().map(|g| g.lane))
                    .collect()
            };
            assert!(lanes_of(a).is_disjoint(&lanes_of(b)), "{a:?} vs {b:?}");
        }
    }

    /// Rows read left-to-right and are labelled on the left; `\` lines read top-left to
    /// bottom-right but are labelled at the bottom, so their clue list is drawn reversed.
    #[test]
    fn gutters_sit_at_the_labelled_end() {
        let geo = Geometry::<Tri>::new(Outline::hexagon(2));
        for (clue_set, lanes) in geo.gutters() {
            let reversed_expected =
                matches!(clue_set, Some(ClueSet::Bottom) | Some(ClueSet::BottomRight));
            for l in lanes {
                assert_eq!(l.reversed, reversed_expected, "{clue_set:?}");
                // The anchor must be next to the end of the lane it belongs to.
                let cells = &geo.lane(l.lane).cells;
                let end = if l.reversed {
                    *cells.last().unwrap()
                } else {
                    *cells.first().unwrap()
                };
                let center = geo.cell_shape(end).center(geo.cell_origin(end));
                let d = ((l.anchor.x - center.x).powi(2) + (l.anchor.y - center.y).powi(2)).sqrt();
                assert!(d < 0.75, "{clue_set:?} anchor is not beside its end cell");
            }
        }
    }

    /// The layout check I can't do by eye: real clue boxes for a real puzzle must sit outside the
    /// grid and must not pile up on each other.
    #[test]
    fn clue_boxes_clear_the_grid_and_each_other() {
        use crate::layout::{CLUE_BOX, GutterLane};

        let outline = Outline::hexagon(3);
        let geo = Geometry::<Tri>::new(outline);

        // A pattern with a decent spread of clue counts.
        let filled: Vec<bool> = (0..geo.cell_count())
            .map(|i| i % 3 != 0 && i % 7 != 1)
            .collect();
        let clue_counts: Vec<usize> = (0..geo.lane_count())
            .map(|lane| {
                let mut runs = 0;
                let mut prev = false;
                for c in &geo.lane(lane).cells {
                    let f = filled[*c as usize];
                    if f && !prev {
                        runs += 1;
                    }
                    prev = f;
                }
                runs
            })
            .collect();

        let mut boxes: Vec<(usize, usize, Point)> = vec![];
        for (_, gutter) in geo.gutters() {
            for g in gutter {
                for i in 0..clue_counts[g.lane] {
                    boxes.push((g.lane, i, g.clue_box_center(i)));
                }
            }
        }
        assert!(boxes.len() > 40, "expected a decent number of clues");

        // No clue box centre may land on a cell.
        for (lane, i, c) in &boxes {
            for cell in 0..geo.cell_count() as u32 {
                assert!(
                    !geo.cell_shape(cell).contains(geo.cell_origin(cell), *c),
                    "clue {i} of lane {lane} at {c:?} sits on top of cell {cell}"
                );
            }
        }

        // Nor may two clue boxes overlap.
        for (i, (la, ia, a)) in boxes.iter().enumerate() {
            for (lb, ib, b) in &boxes[i + 1..] {
                let (dx, dy) = ((a.x - b.x).abs(), (a.y - b.y).abs());
                assert!(
                    dx >= CLUE_BOX - 1e-3 || dy >= CLUE_BOX - 1e-3,
                    "clue {ia} of lane {la} overlaps clue {ib} of lane {lb} ({a:?} vs {b:?})"
                );
            }
        }

        // And the reserved margin must actually contain them.
        let extent = geo.extent();
        let (mut lo, mut hi) = (Point::new(0.0, 0.0), Point::new(extent.x, extent.y));
        for (_, gutter) in geo.gutters() {
            for g in gutter {
                let len = GutterLane::clue_run_length(clue_counts[g.lane]);
                let tip = Point::new(
                    g.anchor.x + g.outward.x * len,
                    g.anchor.y + g.outward.y * len,
                );
                lo.x = lo.x.min(tip.x - CLUE_BOX);
                lo.y = lo.y.min(tip.y - CLUE_BOX);
                hi.x = hi.x.max(tip.x + CLUE_BOX);
                hi.y = hi.y.max(tip.y + CLUE_BOX);
            }
        }
        for (lane, i, c) in &boxes {
            let half = CLUE_BOX / 2.0;
            assert!(
                c.x - half >= lo.x - 1e-3
                    && c.y - half >= lo.y - 1e-3
                    && c.x + half <= hi.x + 1e-3
                    && c.y + half <= hi.y + 1e-3,
                "clue {i} of lane {lane} at {c:?} escapes the reserved area {lo:?}..{hi:?}"
            );
        }
    }

    #[test]
    fn guides_and_gutters_cover_every_lane() {
        for dims in tri_shapes() {
            let geo = Geometry::<Tri>::new(dims);
            let lanes = geo.lane_map().lane_count();
            assert_eq!(geo.guides().len(), lanes);
            assert_eq!(
                geo.gutters().iter().map(|(_, g)| g.len()).sum::<usize>(),
                lanes
            );
            // A triddler labels its lanes with all six webpbn clue sets.
            assert!(geo.gutters().len() <= 6);
        }
        let geo = Geometry::<Square>::new(Rect {
            width: 4,
            height: 3,
        });
        assert_eq!(geo.guides().len(), 7);
        // Square lanes carry no clue set, so they all land in one bucket.
        assert_eq!(geo.gutters().len(), 1);
        assert_eq!(geo.gutters()[0].1.len(), 7);
    }
}
