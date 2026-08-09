//! Abstract drawing geometry: where cells sit, what shape they are, and what was clicked.
//!
//! Everything here is in *abstract units*, in which **every cell edge is exactly 1.0 long**. The
//! GUI multiplies by a scale factor and adds an origin; nothing in this module knows about egui,
//! so it still builds for wasm and the CLI.
//!
//! The y axis points down, matching egui, so a canvas transform is a plain uniform scale with no
//! flip.

/// A position in abstract units.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

/// A displacement in abstract units.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub fn new(x: f32, y: f32) -> Point {
        Point { x, y }
    }
}

impl Vec2 {
    pub fn new(x: f32, y: f32) -> Vec2 {
        Vec2 { x, y }
    }
}

impl std::ops::Add<Vec2> for Point {
    type Output = Point;
    fn add(self, v: Vec2) -> Point {
        Point::new(self.x + v.x, self.y + v.y)
    }
}

/// The height of a row of equilateral triangles with edge 1.0: √3/2.
pub const TRI_ROW_HEIGHT: f32 = 0.866_025_4;

/// Half a triangle's base — the horizontal distance between consecutive cells in a triangular
/// row, since neighbouring triangles overlap by half their width.
pub const TRI_HALF_BASE: f32 = 0.5;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CellShape {
    Square,
    UpTriangle,
    DownTriangle,
}

impl CellShape {
    /// The size of this cell's axis-aligned bounding box.
    pub fn size(self) -> Vec2 {
        match self {
            CellShape::Square => Vec2::new(1.0, 1.0),
            CellShape::UpTriangle | CellShape::DownTriangle => Vec2::new(1.0, TRI_ROW_HEIGHT),
        }
    }

    /// The cell's corners, clockwise, given the top-left of its bounding box. Returns a fixed
    /// array plus how much of it is used, so callers need no allocation.
    pub fn vertices(self, origin: Point) -> ([Point; 4], usize) {
        self.vertices_sized(origin, self.size())
    }

    /// As `vertices`, but for an arbitrary bounding box instead of the unit lattice cell — for
    /// drawing a cell-shaped swatch that isn't itself a grid cell (e.g. the solver sidebar's
    /// rosette, which shows the hovered cell's own shape at UI scale).
    pub fn vertices_sized(self, origin: Point, size: Vec2) -> ([Point; 4], usize) {
        let (x, y) = (origin.x, origin.y);
        let (w, h) = (size.x, size.y);
        match self {
            CellShape::Square => (
                [
                    Point::new(x, y),
                    Point::new(x + w, y),
                    Point::new(x + w, y + h),
                    Point::new(x, y + h),
                ],
                4,
            ),
            // Apex at the top, base along the bottom.
            CellShape::UpTriangle => (
                [
                    Point::new(x + w / 2.0, y),
                    Point::new(x + w, y + h),
                    Point::new(x, y + h),
                    Point::default(),
                ],
                3,
            ),
            // Base along the top, apex at the bottom.
            CellShape::DownTriangle => (
                [
                    Point::new(x, y),
                    Point::new(x + w, y),
                    Point::new(x + w / 2.0, y + h),
                    Point::default(),
                ],
                3,
            ),
        }
    }

    /// The two vertices bounding this cell's edge that borders family `family`.
    ///
    /// A square cell has *two* edges per family (e.g. top and bottom both border family 0, the
    /// rows) — `near` picks which one (true = the edge facing lane 0, i.e. top for rows, left
    /// for columns). A triangular cell has exactly one edge per family (its three edges cover
    /// the three families one each), so `near` is ignored; use [`Self::triangle_edge_is_near`]
    /// to find out in advance which side that single edge is actually on.
    ///
    /// Vertex indices below are verified against `vertices`' documented order, not re-derived by
    /// eye: for an up-triangle `[apex, bottom-right, bottom-left]`, its right edge (apex to
    /// bottom-right) borders whichever family groups by the coordinate that changes across that
    /// edge, and so on for the other two edges and for a down-triangle's `[top-left, top-right,
    /// bottom-apex]`.
    pub fn family_edge(self, origin: Point, family: usize, near: bool) -> (Point, Point) {
        let (p, _) = self.vertices(origin);
        match self {
            CellShape::Square => match (family, near) {
                (0, true) => (p[0], p[1]),  // top
                (0, false) => (p[3], p[2]), // bottom
                (_, true) => (p[3], p[0]),  // left
                (_, false) => (p[1], p[2]), // right
            },
            CellShape::UpTriangle => match family {
                0 => (p[1], p[2]), // bottom
                1 => (p[2], p[0]), // left
                _ => (p[0], p[1]), // right
            },
            CellShape::DownTriangle => match family {
                0 => (p[0], p[1]), // top
                1 => (p[1], p[2]), // right
                _ => (p[2], p[0]), // left
            },
        }
    }

    /// For a triangular cell, whether its single `family_edge` for `family` is on the "near"
    /// (leading, e.g. the top of a row) side rather than the "far" (trailing) side. An
    /// up-triangle's bottom edge leads to the *next* row, so it's a family-0 far edge; its left
    /// edge leads to the previous "/" lane, so it's a family-1 near edge; and so on. Meaningless
    /// for a square, which has both sides on every cell.
    pub fn triangle_edge_is_near(self, family: usize) -> bool {
        match self {
            CellShape::UpTriangle => family == 1,
            CellShape::DownTriangle => family != 1,
            CellShape::Square => true,
        }
    }

    /// The cell's centroid — where a dot, cross, or overlay belongs. Note this is *not* the centre
    /// of the bounding box for a triangle: a triangle's centroid is a third of the way from its
    /// base toward its apex.
    pub fn center(self, origin: Point) -> Point {
        let h = TRI_ROW_HEIGHT;
        match self {
            CellShape::Square => Point::new(origin.x + 0.5, origin.y + 0.5),
            CellShape::UpTriangle => Point::new(origin.x + 0.5, origin.y + 2.0 * h / 3.0),
            CellShape::DownTriangle => Point::new(origin.x + 0.5, origin.y + h / 3.0),
        }
    }

    /// The cell's corners pulled `factor` of the way toward its centroid, for drawing a smaller
    /// swatch inside it (the disambiguation overlay).
    pub fn shrunk(self, origin: Point, factor: f32) -> ([Point; 4], usize) {
        let c = self.center(origin);
        let (mut points, n) = self.vertices(origin);
        for p in points.iter_mut().take(n) {
            p.x = c.x + (p.x - c.x) * factor;
            p.y = c.y + (p.y - c.y) * factor;
        }
        (points, n)
    }

    /// Whether `p` is inside this cell. Only used to check the hit test in tests, but cheap and
    /// generally useful.
    pub fn contains(self, origin: Point, p: Point) -> bool {
        let (points, n) = self.vertices(origin);
        // Convex polygon: inside iff `p` is on the same side of every edge.
        let mut sign = 0.0f32;
        for i in 0..n {
            let (a, b) = (points[i], points[(i + 1) % n]);
            let cross = (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
            if cross != 0.0 {
                if sign == 0.0 {
                    sign = cross.signum();
                } else if cross.signum() != sign {
                    return false;
                }
            }
        }
        true
    }
}

/// One boundary line between lanes, for drawing the grid.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Guide {
    pub from: Point,
    pub to: Point,
    pub family: usize,
    /// Which boundary within the family, counting from 0.
    pub index: usize,
    /// Every fifth line, drawn heavier — the same emphasis square grids have always had.
    pub emphasis: bool,
}

/// A clue box's long side (the extent along its own lane), in abstract units (one cell edge =
/// 1.0).
///
/// Adjacent parallel lanes are `TRI_ROW_HEIGHT` apart, but the boxes are axis-aligned while a
/// diagonal gutter is not: that 0.866 of separation splits into (0.75, 0.43), so the box has to
/// fit inside the *larger* component or neighbouring lanes' clues would still overlap on screen.
pub const CLUE_BOX: f32 = 0.7;
/// A clue box's short side (the extent across the lane, flush against the puzzle edge it's lined
/// up against). Just under one full cell edge, so a chain of boxes reads as an extension of the
/// grid without touching its neighbouring gutter's boxes.
pub const CLUE_BOX_SHORT: f32 = 0.95;
/// The gap between one clue box and the next along a gutter. Chosen so that consecutive boxes on
/// a diagonal gutter clear each other too.
pub const CLUE_GAP: f32 = 0.18;
/// Breathing room between the grid and the nearest clue. Wide enough for the solve view's
/// per-line analysis mark (skim dot, scrub diamond, error cross — radius `0.2 * scale`, so up to
/// 0.4 units across) to sit at its midpoint without touching the grid or the first clue box.
pub const CLUE_PAD: f32 = 0.6;

/// Unit vectors along each triangular family's own lane direction: rows, `/` lines, `\` lines.
const TRI_LANE_DIR: [Vec2; 3] = [
    Vec2 { x: 1.0, y: 0.0 },
    Vec2 {
        x: 0.5,
        y: -TRI_ROW_HEIGHT,
    },
    Vec2 {
        x: 0.5,
        y: TRI_ROW_HEIGHT,
    },
];

/// A clue box shaped like a rhombus pointing along the lane: its long *side* runs along the
/// lane's own direction (i.e. along `outward`), and its short side runs along `edge_dir` — the
/// puzzle boundary edge the box is lined up against — so a chain of clues reads as beads strung
/// along the gutter, each one flush against the grid, rather than a stack of boxes poking into
/// their neighbours at the wrong angle. `size` is the box's extent along the lane's own
/// direction; `short` is its extent across the lane; `edge_dir` need not be perpendicular to
/// `size`'s direction (for a triangular grid it's 60° off).
pub fn tri_clue_rhombus(
    center: Point,
    family: usize,
    edge_dir: Vec2,
    size: f32,
    short: f32,
) -> [Point; 4] {
    let dir = TRI_LANE_DIR[family];
    let (ax, ay) = (dir.x * size / 2.0, dir.y * size / 2.0);
    let (bx, by) = (edge_dir.x * short / 2.0, edge_dir.y * short / 2.0);
    [
        Point::new(center.x + ax + bx, center.y + ay + by),
        Point::new(center.x - ax + bx, center.y - ay + by),
        Point::new(center.x - ax - bx, center.y - ay - by),
        Point::new(center.x + ax - bx, center.y + ay - by),
    ]
}

/// Where one lane's clues should be drawn.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct GutterLane {
    /// Index into `LaneMap::lanes()`.
    pub lane: usize,
    /// The midpoint of the outer edge of the lane's clued end.
    pub anchor: Point,
    /// The unit vector clue boxes march along, pointing away from the grid.
    pub outward: Vec2,
    /// The direction of the puzzle boundary edge the clue chain is lined up against — the short
    /// side of `tri_clue_rhombus`'s boxes runs along this. For a square grid it's perpendicular
    /// to `outward`; for a triangular grid it's 60° off, since that's the only angle a triangle's
    /// edges come in.
    pub edge_dir: Vec2,
    /// Whether the lane's stored clue list must be reversed to read in display order.
    pub reversed: bool,
}

impl GutterLane {
    /// The centre of the `i`th clue box, counting outward from the grid.
    pub fn clue_box_center(&self, i: usize) -> Point {
        let out = CLUE_PAD + CLUE_BOX / 2.0 + i as f32 * (CLUE_BOX + CLUE_GAP);
        Point::new(
            self.anchor.x + self.outward.x * out,
            self.anchor.y + self.outward.y * out,
        )
    }

    /// How far `count` clue boxes reach out from the grid.
    pub fn clue_run_length(count: usize) -> f32 {
        if count == 0 {
            0.0
        } else {
            CLUE_PAD + count as f32 * (CLUE_BOX + CLUE_GAP)
        }
    }
}
