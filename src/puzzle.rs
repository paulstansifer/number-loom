use core::panic;
use std::fmt::Debug;
use std::hash::Hash;
use std::{collections::HashMap, hash::Hasher};

use crate::{
    geometry::{Geometry, GridKind, Outline, Rect, Shape, Square, Tri, TriCoord},
    grid_solve::{self, LineStatus, SolveOptions},
    import::{solution_to_puzzle, solution_to_tri_puzzle, solution_to_triano_puzzle},
};
use serde::{Deserialize, Serialize};
/// A puzzle's colours, always including the background.
pub type Palette = HashMap<Color, ColorInfo>;

pub trait Clue: Clone + Copy + Debug + PartialEq + Eq + Hash + Send {
    fn style() -> ClueStyle;

    fn must_be_separated_from(&self, next: &Self) -> bool;

    fn len(&self) -> usize;

    fn color_at(&self, idx: usize) -> Color;

    // Summary string (for display while solving)
    fn to_string(&self, palette: &Palette) -> String;

    // TODO: these are a hack!
    fn html_color(&self, palette: &Palette) -> String;

    fn html_text(&self, palette: &Palette) -> String;

    fn express<'a>(&self, palette: &'a Palette) -> Vec<(&'a ColorInfo, Option<u16>)>;
}

impl Debug for Nono {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}]{}", self.color.0, self.count)
    }
}

#[derive(PartialEq, Eq, Clone, Copy, Hash, Serialize, Deserialize)]
pub struct Nono {
    pub color: Color,
    pub count: u16,
}

impl Clue for Nono {
    fn style() -> ClueStyle {
        ClueStyle::Nono
    }

    fn must_be_separated_from(&self, next: &Self) -> bool {
        self.color == next.color
    }

    fn len(&self) -> usize {
        self.count as usize
    }
    fn color_at(&self, _: usize) -> Color {
        self.color
    }

    fn to_string(&self, palette: &Palette) -> String {
        format!("{}{}", palette[&self.color].ch, self.count)
    }

    fn html_color(&self, palette: &Palette) -> String {
        let (r, g, b) = palette[&self.color].rgb;
        format!("color:rgb({},{},{})", r, g, b)
    }

    fn html_text(&self, _: &Palette) -> String {
        format!("{}", self.count)
    }

    fn express<'a>(&self, palette: &'a Palette) -> Vec<(&'a ColorInfo, Option<u16>)> {
        vec![(&palette[&self.color], Some(self.count))]
    }
}

#[derive(PartialEq, Eq, Clone, Copy, Hash, Serialize, Deserialize)]
pub struct Triano {
    pub front_cap: Option<Color>,
    pub body_len: u16,
    pub body_color: Color,
    pub back_cap: Option<Color>,
}

impl Clue for Triano {
    fn style() -> ClueStyle {
        ClueStyle::Triano
    }

    fn len(&self) -> usize {
        self.body_len as usize
            + self.front_cap.is_some() as usize
            + self.back_cap.is_some() as usize
    }
    fn color_at(&self, idx: usize) -> Color {
        match (idx, self.front_cap, self.back_cap) {
            (0, Some(c), _) => c,
            (idx, _, Some(c)) if idx == self.len() - 1 => c,
            _ => self.body_color,
        }
    }
    fn must_be_separated_from(&self, next: &Self) -> bool {
        // TODO: check the semantics with the book!
        self.body_color == next.body_color && self.back_cap.is_none() && next.front_cap.is_none()
    }

    fn to_string(&self, palette: &Palette) -> String {
        let mut res = String::new();
        if let Some(front_cap) = self.front_cap {
            res.push_str(&palette[&front_cap].ch.to_string());
        }
        res.push_str(&palette[&self.body_color].ch.to_string());
        res.push_str(&self.body_len.to_string());
        if let Some(back_cap) = self.back_cap {
            res.push_str(&palette[&back_cap].ch.to_string());
        }
        res
    }

    fn html_color(&self, palette: &Palette) -> String {
        let (r, g, b) = palette[&self.body_color].rgb;
        format!("color:rgb({},{},{})", r, g, b)
    }

    fn html_text(&self, palette: &Palette) -> String {
        let mut res = String::new();
        if let Some(front_cap) = self.front_cap {
            let color_info = &palette[&front_cap];
            res.push(color_info.ch);
        }
        res.push_str(&self.body_len.to_string());
        if let Some(back_cap) = self.back_cap {
            let color_info = &palette[&back_cap];
            res.push(color_info.ch);
        }
        res
    }

    fn express<'a>(&self, palette: &'a Palette) -> Vec<(&'a ColorInfo, Option<u16>)> {
        let mut res = vec![];
        if let Some(front_cap) = self.front_cap {
            res.push((&palette[&front_cap], None));
        }
        if self.body_len > 0 {
            res.push((&palette[&self.body_color], Some(self.body_len)));
        }
        if let Some(back_cap) = self.back_cap {
            res.push((&palette[&back_cap], None));
        }
        res
    }
}

impl Debug for Triano {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(front_cap) = self.front_cap {
            write!(f, "[{}]", front_cap.0)?;
        }
        write!(f, "[{}]{}", self.body_color.0, self.body_len)?;
        if let Some(back_cap) = self.back_cap {
            write!(f, "[{}]", back_cap.0)?;
        }
        Ok(())
    }
}

#[derive(PartialEq, Eq, Clone, Copy, Hash, Debug, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Color(pub u8);

pub static BACKGROUND: Color = Color(0);
pub static UNSOLVED: Color = Color(255);

// A triangle-shaped half of a square. `true` means solid in the given direction.
#[derive(PartialEq, Eq, Clone, Copy, Debug, Hash, Serialize, Deserialize)]
pub struct Corner {
    pub upper: bool,
    pub left: bool,
}

// Note that `rgb` is not necessarily unique!
// But `ch` and `name` ought to be, along with `rgb` + `corner`.
#[derive(PartialEq, Eq, Clone, Debug, Hash, Serialize, Deserialize)]
pub struct ColorInfo {
    pub ch: char,
    pub name: String,
    pub rgb: (u8, u8, u8),
    pub color: Color,
    pub corner: Option<Corner>,
}

impl ColorInfo {
    pub fn default_bg() -> ColorInfo {
        ColorInfo {
            ch: ' ',
            name: "white".to_string(),
            rgb: (255, 255, 255),
            color: BACKGROUND,
            corner: None,
        }
    }
    pub fn default_fg(color: Color) -> ColorInfo {
        ColorInfo {
            ch: '#',
            name: "black".to_string(),
            rgb: (0, 0, 0),
            color,
            corner: None,
        }
    }
}

/// A picture: one colour per cell. Cells may be `UNSOLVED`, so this also represents a puzzle
/// part-way through being solved, or an ambiguous one still under development.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Solution<K: GridKind> {
    pub clue_style: ClueStyle,
    pub palette: HashMap<Color, ColorInfo>, // should include the background!
    pub geometry: Geometry<K>,
    /// One entry per cell, in the dense order `geometry` defines. For square puzzles that is
    /// `y * width + x`. Use `at`/`set` when you have `(x, y)` coordinates in hand.
    pub cells: Vec<Color>,
}

// Instead of using the special `UNSOLVED` color, uses masks to represent partial cell information.
//
// Indexed by the dense cell numbering that `Geometry` defines, so it works for any shape. For
// square puzzles that numbering is `y * width + x`, i.e. the same row-major layout the old
// `Array2` had.
pub type PartialSolution = Vec<crate::line_solve::Cell>;

impl<K: GridKind> Solution<K> {
    pub fn new(
        clue_style: ClueStyle,
        palette: HashMap<Color, ColorInfo>,
        geometry: Geometry<K>,
        cells: Vec<Color>,
    ) -> Solution<K> {
        assert_eq!(cells.len(), geometry.cell_count());
        Solution {
            clue_style,
            palette,
            geometry,
            cells,
        }
    }

    /// The colour at a coordinate, or `None` if it is outside the puzzle.
    pub fn get(&self, coord: K::Coord) -> Option<Color> {
        self.geometry.cell(coord).map(|c| self.cells[c as usize])
    }

    pub fn to_partial(&self) -> PartialSolution {
        self.cells
            .iter()
            .map(|color| {
                if *color == UNSOLVED {
                    crate::line_solve::Cell::new_anything()
                } else {
                    crate::line_solve::Cell::from_color(*color)
                }
            })
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Puzzle<C: Clue, K: GridKind> {
    pub palette: HashMap<Color, ColorInfo>, // should include the background!
    pub geometry: Geometry<K>,
    /// One clue list per lane, indexed exactly like `geometry.lane_map().lanes()`. For square
    /// puzzles that means all the rows first, then all the columns; use `row_clues`/`col_clues`.
    pub lines: Vec<Vec<C>>,
}

impl<C: Clue, K: GridKind> Hash for Puzzle<C, K> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.lines.hash(state);
    }
}

impl<C: Clue> Puzzle<C, Square> {
    pub fn square(
        palette: HashMap<Color, ColorInfo>,
        rows: Vec<Vec<C>>,
        cols: Vec<Vec<C>>,
    ) -> Puzzle<C, Square> {
        let geometry = Geometry::new(Rect {
            width: cols.len(),
            height: rows.len(),
        });
        let mut lines = rows;
        lines.extend(cols);
        Puzzle {
            palette,
            geometry,
            lines,
        }
    }

    /// A puzzle with a single lane, for testing line logic without a second dimension
    /// constraining it.
    #[cfg(test)]
    pub fn single_lane(
        palette: HashMap<Color, ColorInfo>,
        len: usize,
        clues: Vec<C>,
    ) -> Puzzle<C, Square> {
        Puzzle {
            palette,
            geometry: Geometry::<Square>::single_lane(len),
            lines: vec![clues],
        }
    }

    pub fn row_clues(&self) -> &[Vec<C>] {
        &self.lines[self.geometry.lane_map().family(0)]
    }

    pub fn col_clues(&self) -> &[Vec<C>] {
        &self.lines[self.geometry.lane_map().family(1)]
    }
}

impl<C: Clue> Puzzle<C, Tri> {
    pub fn triangular(
        palette: HashMap<Color, ColorInfo>,
        outline: Outline,
        lines: Vec<Vec<C>>,
    ) -> Puzzle<C, Tri> {
        let geometry = Geometry::new(outline);
        assert_eq!(
            lines.len(),
            geometry.lane_map().lane_count(),
            "wrong number of clue lists for this outline"
        );
        Puzzle {
            palette,
            geometry,
            lines,
        }
    }
}

impl<C: Clue, K: GridKind> Puzzle<C, K> {
    pub fn lane_map(&self) -> &crate::geometry::LaneMap {
        self.geometry.lane_map()
    }
}

/// A puzzle of whatever kind was loaded.
///
/// The variants enumerate the *valid* combinations of clue style and grid shape. There is no
/// triangular Triano variant because that combination is meaningless — a decision that used to be
/// a runtime check and is now simply unrepresentable.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum DynPuzzle {
    SquareNono(Puzzle<Nono, Square>),
    SquareTriano(Puzzle<Triano, Square>),
    TriNono(Puzzle<Nono, Tri>),
}

impl From<Puzzle<Nono, Square>> for DynPuzzle {
    fn from(p: Puzzle<Nono, Square>) -> DynPuzzle {
        DynPuzzle::SquareNono(p)
    }
}

impl From<Puzzle<Triano, Square>> for DynPuzzle {
    fn from(p: Puzzle<Triano, Square>) -> DynPuzzle {
        DynPuzzle::SquareTriano(p)
    }
}

impl From<Puzzle<Nono, Tri>> for DynPuzzle {
    fn from(p: Puzzle<Nono, Tri>) -> DynPuzzle {
        DynPuzzle::TriNono(p)
    }
}

/// A picture of whatever kind was loaded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DynSolution {
    Square(Solution<Square>),
    Tri(Solution<Tri>),
}

/// A cell coordinate of whatever kind the puzzle uses.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum DynCoord {
    Square((usize, usize)),
    Tri(TriCoord),
}

/// Run the same body against whichever `Puzzle` a `DynPuzzle` holds.
///
/// This replaces the old two-closure `specialize`, which cannot express this: a closure can't be
/// generic over both the clue type and the grid kind.
#[macro_export]
macro_rules! with_puzzle {
    ($dyn_puzzle:expr, |$p:ident| $body:expr) => {
        match $dyn_puzzle {
            $crate::puzzle::DynPuzzle::SquareNono($p) => $body,
            $crate::puzzle::DynPuzzle::SquareTriano($p) => $body,
            $crate::puzzle::DynPuzzle::TriNono($p) => $body,
        }
    };
}

/// Run the same body against whichever `Solution` a `DynSolution` holds.
#[macro_export]
macro_rules! with_solution {
    ($dyn_solution:expr, |$s:ident| $body:expr) => {
        match $dyn_solution {
            $crate::puzzle::DynSolution::Square($s) => $body,
            $crate::puzzle::DynSolution::Tri($s) => $body,
        }
    };
}

pub trait PuzzleDynOps {
    fn palette(&self) -> &HashMap<Color, ColorInfo>;
    /// How many lanes each clue family has: two entries for a square puzzle, three for a triddler.
    fn family_lane_counts(&self) -> Vec<usize>;
    /// The puzzle's bounding box in abstract units, for sizing a canvas.
    fn extent(&self) -> crate::layout::Vec2;
    fn solve(
        &self,
        options: &crate::grid_solve::SolveOptions,
    ) -> anyhow::Result<crate::grid_solve::Report>;
    fn partial_solve(
        &self,
        partial: &mut PartialSolution,
        options: &crate::grid_solve::SolveOptions,
    ) -> anyhow::Result<crate::grid_solve::Report>;
    fn plain_solve(&self) -> anyhow::Result<crate::grid_solve::Report> {
        self.solve(&SolveOptions::default())
    }
    fn analyze_lines(&self, partial: &PartialSolution) -> (Vec<LineStatus>, Vec<LineStatus>);
    fn settle_solution(&self, partial: &mut PartialSolution) -> anyhow::Result<()>;
}

impl<C: Clue, K: GridKind> PuzzleDynOps for Puzzle<C, K> {
    fn palette(&self) -> &HashMap<Color, ColorInfo> {
        &self.palette
    }

    fn family_lane_counts(&self) -> Vec<usize> {
        let lanes = self.geometry.lane_map();
        (0..lanes.family_count())
            .map(|f| lanes.family(f).len())
            .collect()
    }

    fn extent(&self) -> crate::layout::Vec2 {
        self.geometry.extent()
    }

    fn partial_solve(
        &self,
        partial: &mut PartialSolution,
        options: &crate::grid_solve::SolveOptions,
    ) -> anyhow::Result<crate::grid_solve::Report> {
        grid_solve::solve_grid(self, &mut None, options, partial)
    }

    fn solve(&self, options: &SolveOptions) -> anyhow::Result<crate::grid_solve::Report> {
        let mut partial =
            vec![crate::line_solve::Cell::new(&self.palette); self.geometry.cell_count()];

        grid_solve::solve_grid(self, &mut None, options, &mut partial)
    }

    fn analyze_lines(&self, partial: &PartialSolution) -> (Vec<LineStatus>, Vec<LineStatus>) {
        grid_solve::analyze_lines(self, partial)
    }

    fn settle_solution(&self, partial: &mut PartialSolution) -> anyhow::Result<()> {
        grid_solve::settle_solution(self, partial)
    }
}

impl PuzzleDynOps for DynPuzzle {
    fn palette(&self) -> &HashMap<Color, ColorInfo> {
        with_puzzle!(self, |p| p.palette())
    }

    fn family_lane_counts(&self) -> Vec<usize> {
        with_puzzle!(self, |p| p.family_lane_counts())
    }

    fn extent(&self) -> crate::layout::Vec2 {
        with_puzzle!(self, |p| p.extent())
    }

    fn partial_solve(
        &self,
        partial: &mut PartialSolution,
        options: &crate::grid_solve::SolveOptions,
    ) -> anyhow::Result<crate::grid_solve::Report> {
        with_puzzle!(self, |p| p.partial_solve(partial, options))
    }

    fn solve(
        &self,
        options: &crate::grid_solve::SolveOptions,
    ) -> anyhow::Result<crate::grid_solve::Report> {
        with_puzzle!(self, |p| p.solve(options))
    }

    fn analyze_lines(&self, partial: &PartialSolution) -> (Vec<LineStatus>, Vec<LineStatus>) {
        with_puzzle!(self, |p| p.analyze_lines(partial))
    }

    fn settle_solution(&self, partial: &mut PartialSolution) -> anyhow::Result<()> {
        with_puzzle!(self, |p| p.settle_solution(partial))
    }
}

impl DynPuzzle {
    pub fn shape(&self) -> Shape {
        with_puzzle!(self, |p| p.geometry.shape())
    }

    pub fn clue_lines(&self) -> usize {
        with_puzzle!(self, |p| p.lines.len())
    }

    pub fn as_square_nono(&self) -> Option<&Puzzle<Nono, Square>> {
        match self {
            DynPuzzle::SquareNono(p) => Some(p),
            _ => None,
        }
    }

    pub fn as_square_triano(&self) -> Option<&Puzzle<Triano, Square>> {
        match self {
            DynPuzzle::SquareTriano(p) => Some(p),
            _ => None,
        }
    }

    pub fn as_tri_nono(&self) -> Option<&Puzzle<Nono, Tri>> {
        match self {
            DynPuzzle::TriNono(p) => Some(p),
            _ => None,
        }
    }
}

impl DynSolution {
    pub fn palette(&self) -> &HashMap<Color, ColorInfo> {
        with_solution!(self, |s| &s.palette)
    }

    pub fn palette_mut(&mut self) -> &mut HashMap<Color, ColorInfo> {
        with_solution!(self, |s| &mut s.palette)
    }

    pub fn cells(&self) -> &[Color] {
        with_solution!(self, |s| &s.cells)
    }

    pub fn cells_mut(&mut self) -> &mut Vec<Color> {
        with_solution!(self, |s| &mut s.cells)
    }

    pub fn clue_style(&self) -> ClueStyle {
        with_solution!(self, |s| s.clue_style)
    }

    pub fn shape(&self) -> Shape {
        with_solution!(self, |s| s.geometry.shape())
    }

    pub fn lane_map(&self) -> &crate::geometry::LaneMap {
        with_solution!(self, |s| s.geometry.lane_map())
    }

    pub fn to_partial(&self) -> PartialSolution {
        with_solution!(self, |s| s.to_partial())
    }

    pub fn to_puzzle(&self) -> DynPuzzle {
        match self {
            DynSolution::Square(s) => s.to_puzzle(),
            DynSolution::Tri(s) => s.to_puzzle(),
        }
    }

    pub fn quality_check(&self) -> Vec<String> {
        with_solution!(self, |s| s.quality_check())
    }

    pub fn extent(&self) -> crate::layout::Vec2 {
        with_solution!(self, |s| s.geometry.extent())
    }

    /// The dense cell index for a coordinate, or `None` if the coordinate is the wrong kind or
    /// outside the puzzle.
    pub fn cell_of(&self, coord: DynCoord) -> Option<u32> {
        match (self, coord) {
            (DynSolution::Square(s), DynCoord::Square(c)) => s.geometry.cell(c),
            (DynSolution::Tri(s), DynCoord::Tri(c)) => s.geometry.cell(c),
            _ => None,
        }
    }

    /// Which cell contains this point, in abstract units.
    pub fn cell_at(&self, p: crate::layout::Point) -> Option<DynCoord> {
        match self {
            DynSolution::Square(s) => s.geometry.cell_at(p).map(DynCoord::Square),
            DynSolution::Tri(s) => s.geometry.cell_at(p).map(DynCoord::Tri),
        }
    }

    /// Cells sharing an edge with this one: 4 for a square, 3 for a triangle.
    pub fn neighbor_cells(&self, cell: u32) -> Vec<u32> {
        with_solution!(self, |s| s.geometry.neighbor_cells(cell).collect())
    }

    /// Unit vectors toward each neighbouring lane direction, as `(backward, forward)` pairs per
    /// family — matching what `runs_at_cell` returns.
    pub fn arm_directions(&self) -> &'static [crate::layout::Vec2] {
        with_solution!(self, |s| s.geometry.arm_directions())
    }

    /// Boundary lines between lanes, in abstract units, for drawing the grid.
    pub fn guides(&self) -> &[crate::layout::Guide] {
        with_solution!(self, |s| s.geometry.guides())
    }

    /// How far a run of the same colour extends from a cell along each clue family.
    pub fn runs_at_cell(&self, cell: u32) -> Vec<(usize, usize)> {
        with_solution!(self, |s| {
            let target = s.cells[cell as usize];
            s.geometry.runs(cell, |c| s.cells[c as usize] == target)
        })
    }

    /// The square picture, or `None` for a triddler. The one place code that only understands
    /// rows and columns is allowed to narrow.
    pub fn as_square(&self) -> Option<&Solution<Square>> {
        match self {
            DynSolution::Square(s) => Some(s),
            DynSolution::Tri(_) => None,
        }
    }

    pub fn as_square_mut(&mut self) -> Option<&mut Solution<Square>> {
        match self {
            DynSolution::Square(s) => Some(s),
            DynSolution::Tri(_) => None,
        }
    }
}

/// Line caches are keyed on the clue list plus the gathered lane contents, and geometry never
/// enters that key — so square and triangular `Nono` puzzles correctly share one cache.
pub struct DynSolveCache {
    nono_cache: Option<crate::grid_solve::LineCache<Nono>>,
    triano_cache: Option<crate::grid_solve::LineCache<Triano>>,
}

impl DynSolveCache {
    pub fn new() -> Self {
        DynSolveCache {
            nono_cache: Some(HashMap::new()),
            triano_cache: Some(HashMap::new()),
        }
    }

    pub fn solve(&mut self, p: &DynPuzzle) -> anyhow::Result<crate::grid_solve::Report> {
        let options = crate::grid_solve::SolveOptions::default();
        match p {
            DynPuzzle::SquareNono(p) => crate::grid_solve::solve(p, &mut self.nono_cache, &options),
            DynPuzzle::TriNono(p) => crate::grid_solve::solve(p, &mut self.nono_cache, &options),
            DynPuzzle::SquareTriano(p) => {
                crate::grid_solve::solve(p, &mut self.triano_cache, &options)
            }
        }
    }
}

/// Indexing by coordinate. One impl covers both kinds, because `K::Coord` is determined by `K` —
/// so `sol[(3, 4)]` and `sol[TriCoord::new(a, b, c)]` both work, and so does generic code.
impl<K: GridKind> std::ops::Index<K::Coord> for Solution<K> {
    type Output = Color;

    fn index(&self, coord: K::Coord) -> &Color {
        let cell = self
            .geometry
            .cell(coord)
            .unwrap_or_else(|| panic!("{coord:?} is outside the puzzle"));
        &self.cells[cell as usize]
    }
}

impl<K: GridKind> std::ops::IndexMut<K::Coord> for Solution<K> {
    fn index_mut(&mut self, coord: K::Coord) -> &mut Color {
        let cell = self
            .geometry
            .cell(coord)
            .unwrap_or_else(|| panic!("{coord:?} is outside the puzzle"));
        &mut self.cells[cell as usize]
    }
}

impl Solution<Square> {
    /// Build a square solution from the column-major `grid[x][y]` layout this type once had.
    pub fn from_columns(
        clue_style: ClueStyle,
        palette: HashMap<Color, ColorInfo>,
        grid: Vec<Vec<Color>>,
    ) -> Solution<Square> {
        let width = grid.len();
        let height = grid.first().map(|c| c.len()).unwrap_or(0);
        let mut cells = vec![BACKGROUND; width * height];
        for (x, col) in grid.iter().enumerate() {
            assert_eq!(col.len(), height, "ragged grid");
            for (y, color) in col.iter().enumerate() {
                cells[y * width + x] = *color;
            }
        }
        Solution::new(
            clue_style,
            palette,
            Geometry::new(Rect { width, height }),
            cells,
        )
    }

    /// The inverse of `from_columns`.
    pub fn to_columns(&self) -> Vec<Vec<Color>> {
        (0..self.x_size())
            .map(|x| (0..self.y_size()).map(|y| self[(x, y)]).collect())
            .collect()
    }

    pub fn x_size(&self) -> usize {
        self.geometry.dims().width
    }

    pub fn y_size(&self) -> usize {
        self.geometry.dims().height
    }
}

impl<K: GridKind> Solution<K> {
    pub fn quality_check(&self) -> Vec<String> {
        let mut problems = vec![];
        // Not `x_size`/`y_size`: a triddler has no width or height, but the heuristics below
        // only need a sense of scale, and cell count plus lane count gives that for any shape.
        let cell_count = self.cells.len();
        let lane_count = self.geometry.lane_map().lane_count();

        let bg_squares_found: usize = self.cells.iter().filter(|c| **c == BACKGROUND).count();

        if bg_squares_found < lane_count {
            problems.push(format!(
                "{} is a very small number of background squares",
                bg_squares_found
            ));
        }

        if (cell_count - bg_squares_found) < lane_count {
            problems.push(format!(
                "{} is a very small number of foreground squares",
                cell_count - bg_squares_found
            ));
        }

        let num_colors = self.palette.len();
        if num_colors > 10 {
            problems.push(format!(
                "{} colors detected; that's probably too many.",
                num_colors
            ))
        }

        // Find similar colors
        for (color_key, color) in &self.palette {
            for (color_key2, color2) in &self.palette {
                if color_key >= color_key2 {
                    continue;
                }
                if color.corner != color2.corner && color.rgb == color2.rgb {
                    continue; // Corners may be the same color.
                }
                let (r, g, b) = color.rgb;
                let (r2, g2, b2) = color2.rgb;
                if (r2 as i16 - r as i16).abs()
                    + (g2 as i16 - g as i16).abs()
                    + (b2 as i16 - b as i16).abs()
                    < 30
                {
                    problems.push(format!(
                        "very similar colors found: {:?} (\"{}\") and {:?} (\"{}\")",
                        color.rgb, color.name, color2.rgb, color2.name
                    ));
                }
            }
        }
        problems
    }

    /// How far a run of the same colour extends from `coord` along each clue family, as
    /// `(backward, forward)` counts not including the cell itself.
    pub fn runs_at(&self, coord: K::Coord) -> Vec<(usize, usize)> {
        let Some(cell) = self.geometry.cell(coord) else {
            return vec![];
        };
        let target = self.cells[cell as usize];
        self.geometry
            .runs(cell, |c| self.cells[c as usize] == target)
    }
}

impl Solution<Square> {
    pub fn blank_bw(x_size: usize, y_size: usize) -> Solution<Square> {
        Solution::new(
            ClueStyle::Nono,
            HashMap::from([
                (BACKGROUND, ColorInfo::default_bg()),
                (Color(1), ColorInfo::default_fg(Color(1))),
            ]),
            Geometry::new(Rect {
                width: x_size,
                height: y_size,
            }),
            vec![BACKGROUND; x_size * y_size],
        )
    }

    pub fn to_puzzle(&self) -> DynPuzzle {
        match self.clue_style {
            ClueStyle::Nono => solution_to_puzzle(self).into(),
            ClueStyle::Triano => solution_to_triano_puzzle(self).into(),
        }
    }

    /// Run lengths up, down, left and right of a cell.
    pub fn count_contiguous(&self, x: usize, y: usize) -> (usize, usize, usize, usize) {
        let runs = self.runs_at((x, y));
        let (left, right) = runs[0];
        let (up, down) = runs[1];
        (up, down, left, right)
    }
}

impl Solution<Tri> {
    pub fn to_puzzle(&self) -> DynPuzzle {
        assert_eq!(
            self.clue_style,
            ClueStyle::Nono,
            "a triddler can't have trianogram clues"
        );
        solution_to_tri_puzzle(self).into()
    }
}

#[derive(Clone, Copy, Debug, clap::ValueEnum, Default, PartialEq, Eq)]
pub enum NonogramFormat {
    #[default]
    /// Any image supported by the `image` crate (when used as output, infers format from
    /// extension).
    Image,
    /// The widely-used format associated with http://webpbn.com.
    Webpbn,
    /// The format used by the 'olsak' solver.
    Olsak,
    /// Informal text format: a grid of characters. Attempts some sensible matching of characters
    /// to colors, but results will vary. This is the only format that supports Triano puzzles.
    CharGrid,
    /// Number Loom's format, mostly aimed at making copy-and-paste easier.
    Woven,
    /// (Export-only.) An HTML representation of a puzzle.
    Html,
}

#[derive(Clone, Copy, Debug, clap::ValueEnum, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClueStyle {
    #[default]
    Nono,
    Triano,
}

// `path` may be either a filename or a path
pub fn infer_format(path: &str, format_arg: Option<NonogramFormat>) -> NonogramFormat {
    if let Some(format) = format_arg {
        return format;
    }

    let ext = path.rsplit_once('.').map(|x| x.1);

    match ext {
        Some("png") | Some("bmp") | Some("gif") => NonogramFormat::Image,
        Some("xml") | Some("pbn") => NonogramFormat::Webpbn,
        Some("g") => NonogramFormat::Olsak,
        Some("html") => NonogramFormat::Html,
        Some("txt") => NonogramFormat::CharGrid,
        Some("woven") => NonogramFormat::Woven,
        _ => NonogramFormat::CharGrid,
    }
}

#[derive(Clone, Debug)]
pub struct Document {
    p: Option<DynPuzzle>,
    s: Option<DynSolution>,
    /// Path if native, just a filename, if on the Web
    pub file: String,
    pub title: String,
    pub description: String,
    pub author: String,
    pub id: String,
    pub license: String,
}

impl Document {
    pub fn quality_check(&mut self) -> Vec<String> {
        let mut problems = vec![];
        if self.author.is_empty() {
            problems.push("missing author".to_string());
        }

        if let Ok(solution) = self.solution() {
            problems.extend(solution.quality_check());
        }

        let puzzle = self.puzzle();
        match puzzle.plain_solve() {
            Ok(report) => {
                if report.cells_left > 0 {
                    problems.push(format!("puzzle is not solveable with line-logic"));
                }
            }
            Err(_) => {
                problems.push("puzzle is self-contradictory".to_string());
            }
        }

        problems
    }

    pub fn new(
        puzzle: Option<DynPuzzle>,
        solution: Option<DynSolution>,
        file: String,
        title: Option<String>,
        description: Option<String>,
        author: Option<String>,
        id: Option<String>,
        license: Option<String>,
    ) -> Document {
        assert!(puzzle.is_some() || solution.is_some());
        Document {
            p: puzzle,
            s: solution,
            file,
            title: title.unwrap_or_default(),
            description: description.unwrap_or_default(),
            author: author.unwrap_or_default(),
            id: id.unwrap_or_default(),
            license: license.unwrap_or_default(),
        }
    }

    #[allow(dead_code)] // it's a little weird how this is easy to get but never used
    pub fn file(&self) -> &str {
        &self.file
    }

    pub fn get_or_make_up_title(&self) -> anyhow::Result<String> {
        if !self.title.is_empty() {
            return Ok(self.title.clone());
        }

        let mut hasher = std::hash::DefaultHasher::new();

        if let Some(solution) = self.try_solution() {
            for color in solution.cells() {
                color.hash(&mut hasher);
            }
        } else {
            let puzzle = self.try_puzzle().unwrap();
            puzzle.hash(&mut hasher);
        }

        let hash = hasher.finish().to_le_bytes();

        Ok(mnemonic::to_string(&hash[0..4]))
    }

    #[allow(dead_code)]
    pub fn try_puzzle(&self) -> Option<&DynPuzzle> {
        self.p.as_ref()
    }

    // Returns true if a solution is present or can uniquely be solved from the puzzle,
    // and thus is editable.
    // (So an ambiguous puzzle saved in a format that saved the solution will return true)
    pub fn has_complete_solution(&mut self) -> anyhow::Result<bool> {
        let solution = self.solution()?;

        for &color in solution.cells() {
            if color == UNSOLVED {
                return Ok(false);
            }
        }

        Ok(true)
    }

    /// A rough width and height, for labels and thumbnails. A triddler has neither, so it
    /// reports the sizes of its first two clue families instead.
    pub fn dimensions(&self) -> (usize, usize) {
        if let Some(DynSolution::Square(s)) = &self.s {
            return (s.x_size(), s.y_size());
        }
        let counts = match (&self.s, &self.p) {
            (Some(s), _) => {
                let lanes = s.lane_map();
                (0..lanes.family_count())
                    .map(|f| lanes.family(f).len())
                    .collect()
            }
            (_, Some(p)) => p.family_lane_counts(),
            _ => vec![0, 0],
        };
        (counts[1], counts[0])
    }

    pub fn puzzle(&mut self) -> &DynPuzzle {
        if self.p.is_none() {
            self.p = Some(self.s.as_ref().unwrap().to_puzzle());
        }
        self.p.as_ref().unwrap()
    }

    pub fn try_solution(&self) -> Option<&DynSolution> {
        self.s.as_ref()
    }

    pub fn solution(&mut self) -> anyhow::Result<&DynSolution> {
        if self.s.is_none() {
            self.s = Some(self.p.as_ref().unwrap().plain_solve()?.solution)
        }
        Ok(self.s.as_ref().unwrap())
    }

    pub fn solution_mut(&mut self) -> &mut DynSolution {
        if self.s.is_none() {
            self.s = Some(self.p.as_ref().unwrap().plain_solve().unwrap().solution)
        }
        self.p = None; // Edits will invalidate the puzzle!
        self.s.as_mut().unwrap()
    }

    pub fn take_solution(self) -> anyhow::Result<DynSolution> {
        match self.s {
            Some(s) => Ok(s),
            None => self.p.unwrap().plain_solve().map(|r| r.solution),
        }
    }

    /// The square picture, for code that only understands rows and columns. Callers should
    /// report this to the user rather than unwrapping it — the editor cannot yet edit triddlers.
    pub fn square_solution_mut(&mut self) -> Option<&mut Solution<Square>> {
        self.solution_mut().as_square_mut()
    }

    pub fn from_puzzle(puzzle: DynPuzzle, file: String) -> Self {
        Self {
            p: Some(puzzle),
            s: None,
            file,
            title: "".to_string(),
            description: "".to_string(),
            author: "".to_string(),
            id: "".to_string(),
            license: "".to_string(),
        }
    }

    pub fn from_solution(solution: DynSolution, file: String) -> Self {
        Self {
            p: None,
            s: Some(solution),
            file,
            title: "".to_string(),
            description: "".to_string(),
            author: "".to_string(),
            id: "".to_string(),
            license: "".to_string(),
        }
    }
}
