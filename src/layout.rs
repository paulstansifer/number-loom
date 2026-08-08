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
        let (x, y) = (origin.x, origin.y);
        let h = TRI_ROW_HEIGHT;
        match self {
            CellShape::Square => (
                [
                    Point::new(x, y),
                    Point::new(x + 1.0, y),
                    Point::new(x + 1.0, y + 1.0),
                    Point::new(x, y + 1.0),
                ],
                4,
            ),
            // Apex at the top, base along the bottom.
            CellShape::UpTriangle => (
                [
                    Point::new(x + 0.5, y),
                    Point::new(x + 1.0, y + h),
                    Point::new(x, y + h),
                    Point::default(),
                ],
                3,
            ),
            // Base along the top, apex at the bottom.
            CellShape::DownTriangle => (
                [
                    Point::new(x, y),
                    Point::new(x + 1.0, y),
                    Point::new(x + 0.5, y + h),
                    Point::default(),
                ],
                3,
            ),
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

/// Where one lane's clues should be drawn.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct GutterLane {
    /// Index into `LaneMap::lanes()`.
    pub lane: usize,
    /// The midpoint of the outer edge of the lane's clued end.
    pub anchor: Point,
    /// The unit vector clue boxes march along, pointing away from the grid.
    pub outward: Vec2,
}
