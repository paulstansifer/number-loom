use core::panic;
use std::fmt::Debug;
use std::hash::Hash;
use std::{collections::HashMap, hash::Hasher};

use crate::{
    grid_solve::{self, LineStatus, SolveOptions},
    import::{solution_to_puzzle, solution_to_triano_puzzle},
};
use serde::{Deserialize, Serialize};
pub trait Clue: Clone + Copy + Debug + PartialEq + Eq + Hash + Send {
    fn style() -> ClueStyle;

    fn must_be_separated_from(&self, next: &Self) -> bool;

    fn len(&self) -> usize;

    fn color_at(&self, idx: usize) -> Color;

    // Summary string (for display while solving)
    fn to_string(&self, puzzle: &Puzzle<Self>) -> String;

    // TODO: these are a hack!
    fn html_color(&self, puzzle: &Puzzle<Self>) -> String;

    fn html_text(&self, puzzle: &Puzzle<Self>) -> String;

    fn to_dyn(puzzle: Puzzle<Self>) -> DynPuzzle;

    fn express<'a>(&self, puzzle: &'a Puzzle<Self>) -> Vec<(&'a ColorInfo, Option<u16>)>;
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

    fn to_string(&self, puzzle: &Puzzle<Self>) -> String {
        format!("{}{}", puzzle.palette[&self.color].ch, self.count)
    }

    fn html_color(&self, puzzle: &Puzzle<Self>) -> String {
        let (r, g, b) = puzzle.palette[&self.color].rgb;
        format!("color:rgb({},{},{})", r, g, b)
    }

    fn html_text(&self, _: &Puzzle<Self>) -> String {
        format!("{}", self.count)
    }

    fn to_dyn(puzzle: Puzzle<Self>) -> DynPuzzle {
        DynPuzzle::Nono(puzzle)
    }

    fn express<'a>(&self, puzzle: &'a Puzzle<Self>) -> Vec<(&'a ColorInfo, Option<u16>)> {
        vec![(&puzzle.palette[&self.color], Some(self.count))]
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

    fn to_string(&self, puzzle: &Puzzle<Self>) -> String {
        let mut res = String::new();
        if let Some(front_cap) = self.front_cap {
            res.push_str(&puzzle.palette[&front_cap].ch.to_string());
        }
        res.push_str(&puzzle.palette[&self.body_color].ch.to_string());
        res.push_str(&self.body_len.to_string());
        if let Some(back_cap) = self.back_cap {
            res.push_str(&puzzle.palette[&back_cap].ch.to_string());
        }
        res
    }

    fn html_color(&self, puzzle: &Puzzle<Self>) -> String {
        let (r, g, b) = puzzle.palette[&self.body_color].rgb;
        format!("color:rgb({},{},{})", r, g, b)
    }

    fn html_text(&self, puzzle: &Puzzle<Self>) -> String {
        let mut res = String::new();
        if let Some(front_cap) = self.front_cap {
            let color_info = &puzzle.palette[&front_cap];
            res.push(color_info.ch);
        }
        res.push_str(&self.body_len.to_string());
        if let Some(back_cap) = self.back_cap {
            let color_info = &puzzle.palette[&back_cap];
            res.push(color_info.ch);
        }
        res
    }

    fn to_dyn(puzzle: Puzzle<Self>) -> DynPuzzle {
        DynPuzzle::Triano(puzzle)
    }

    fn express<'a>(&self, puzzle: &'a Puzzle<Self>) -> Vec<(&'a ColorInfo, Option<u16>)> {
        let mut res = vec![];
        if let Some(front_cap) = self.front_cap {
            res.push((&puzzle.palette[&front_cap], None));
        }
        if self.body_len > 0 {
            res.push((&puzzle.palette[&self.body_color], Some(self.body_len)));
        }
        if let Some(back_cap) = self.back_cap {
            res.push((&puzzle.palette[&back_cap], None));
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Solution {
    pub clue_style: ClueStyle,
    pub palette: HashMap<Color, ColorInfo>, // should include the background!
    pub grid: Vec<Vec<Color>>,
}

// Instead of using the special `UNSOLVED` color, uses masks to represent partial cell information.
pub type PartialSolution = ndarray::Array2<crate::line_solve::Cell>;

impl Solution {
    pub fn to_partial(&self) -> PartialSolution {
        let mut res = PartialSolution::from_elem(
            (self.y_size(), self.x_size()),
            crate::line_solve::Cell::new_impossible(),
        );
        for (x, col) in self.grid.iter().enumerate() {
            for (y, color) in col.iter().enumerate() {
                if *color == UNSOLVED {
                    res[[y, x]] = crate::line_solve::Cell::new_anything();
                } else {
                    res[[y, x]] = crate::line_solve::Cell::from_color(*color);
                }
            }
        }
        res
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Puzzle<C: Clue> {
    pub palette: HashMap<Color, ColorInfo>, // should include the background!
    pub rows: Vec<Vec<C>>,
    pub cols: Vec<Vec<C>>,
}

impl<C: Clue> Hash for Puzzle<C> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.rows.hash(state);
        self.cols.hash(state);
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum DynPuzzle {
    Nono(Puzzle<Nono>),
    Triano(Puzzle<Triano>),
}

pub trait PuzzleDynOps {
    fn palette(&self) -> &HashMap<Color, ColorInfo>;
    fn rows(&self) -> usize;
    fn cols(&self) -> usize;
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

impl<C: Clue> PuzzleDynOps for Puzzle<C> {
    fn palette(&self) -> &HashMap<Color, ColorInfo> {
        &self.palette
    }

    fn rows(&self) -> usize {
        self.rows.len()
    }

    fn cols(&self) -> usize {
        self.cols.len()
    }

    fn partial_solve(
        &self,
        partial: &mut PartialSolution,
        options: &crate::grid_solve::SolveOptions,
    ) -> anyhow::Result<crate::grid_solve::Report> {
        grid_solve::solve_grid(self, &mut None, options, partial)
    }

    fn solve(&self, options: &SolveOptions) -> anyhow::Result<crate::grid_solve::Report> {
        let mut partial = PartialSolution::from_elem(
            (self.rows.len(), self.cols.len()),
            crate::line_solve::Cell::new(self),
        );

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
    // Here comes the most inane `impl` you've ever seen!
    fn palette(&self) -> &HashMap<Color, ColorInfo> {
        match self {
            DynPuzzle::Nono(p) => &p.palette(),
            DynPuzzle::Triano(p) => &p.palette(),
        }
    }

    fn rows(&self) -> usize {
        match self {
            DynPuzzle::Nono(p) => p.rows(),
            DynPuzzle::Triano(p) => p.rows(),
        }
    }

    fn cols(&self) -> usize {
        match self {
            DynPuzzle::Nono(p) => p.cols(),
            DynPuzzle::Triano(p) => p.cols(),
        }
    }

    fn partial_solve(
        &self,
        partial: &mut PartialSolution,
        options: &crate::grid_solve::SolveOptions,
    ) -> anyhow::Result<crate::grid_solve::Report> {
        match self {
            DynPuzzle::Nono(p) => p.partial_solve(partial, options),
            DynPuzzle::Triano(p) => p.partial_solve(partial, options),
        }
    }

    fn solve(
        &self,
        options: &crate::grid_solve::SolveOptions,
    ) -> anyhow::Result<crate::grid_solve::Report> {
        match self {
            DynPuzzle::Nono(p) => p.solve(options),
            DynPuzzle::Triano(p) => p.solve(options),
        }
    }

    fn analyze_lines(&self, partial: &PartialSolution) -> (Vec<LineStatus>, Vec<LineStatus>) {
        match self {
            DynPuzzle::Nono(p) => p.analyze_lines(partial),
            DynPuzzle::Triano(p) => p.analyze_lines(partial),
        }
    }

    fn settle_solution(&self, partial: &mut PartialSolution) -> anyhow::Result<()> {
        match self {
            DynPuzzle::Nono(p) => p.settle_solution(partial),
            DynPuzzle::Triano(p) => p.settle_solution(partial),
        }
    }
}

impl DynPuzzle {
    pub fn specialize<FN, FT, T>(&self, f_n: FN, f_t: FT) -> T
    where
        FN: FnOnce(&Puzzle<Nono>) -> T,
        FT: FnOnce(&Puzzle<Triano>) -> T,
    {
        match self {
            DynPuzzle::Nono(p) => f_n(p),
            DynPuzzle::Triano(p) => f_t(p),
        }
    }

    pub fn assume_nono(&self) -> &Puzzle<Nono> {
        match self {
            DynPuzzle::Nono(p) => p,
            DynPuzzle::Triano(_) => panic!("must be a true nonogram"),
        }
    }

    pub fn assume_triano(&self) -> &Puzzle<Triano> {
        match self {
            DynPuzzle::Nono(_) => panic!("must be a trianogram"),
            DynPuzzle::Triano(p) => p,
        }
    }
}

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
        p.specialize(
            |p| crate::grid_solve::solve(p, &mut self.nono_cache, &options),
            |p| crate::grid_solve::solve(p, &mut self.triano_cache, &options),
        )
    }
}

impl Solution {
    pub fn quality_check(&self) -> Vec<String> {
        let mut problems = vec![];
        let width = self.grid.len();
        let height = self.grid.first().unwrap().len();

        let bg_squares_found: usize = self
            .grid
            .iter()
            .map(|col| {
                col.iter()
                    .map(|c| if *c == BACKGROUND { 1 } else { 0 })
                    .sum::<usize>()
            })
            .sum();

        if bg_squares_found < (width + height) {
            problems.push(format!(
                "warning: {} is a very small number of background squares",
                bg_squares_found
            ));
        }

        if (width * height - bg_squares_found) < (width + height) {
            problems.push(format!(
                "warning: {} is a very small number of foreground squares",
                width * height - bg_squares_found
            ));
        }

        let num_colors = self.palette.len();
        if num_colors > 30 {
            problems.push(format!(
                "{} colors detected. Nonograms with more than 30 colors are not supported.",
                num_colors
            ));
        } else if num_colors > 10 {
            problems.push(format!(
                "{} colors detected. That's probably too many.",
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
                        "warning: very similar colors found: {:?} and {:?}",
                        color.rgb, color2.rgb
                    ));
                }
            }
        }
        problems
    }

    pub fn blank_bw(x_size: usize, y_size: usize) -> Solution {
        Solution {
            clue_style: ClueStyle::Nono,
            palette: HashMap::from([
                (BACKGROUND, ColorInfo::default_bg()),
                (Color(1), ColorInfo::default_fg(Color(1))),
            ]),
            grid: vec![vec![BACKGROUND; y_size]; x_size],
        }
    }

    pub fn to_puzzle(&self) -> DynPuzzle {
        match self.clue_style {
            ClueStyle::Nono => DynPuzzle::Nono(solution_to_puzzle(self)),
            ClueStyle::Triano => DynPuzzle::Triano(solution_to_triano_puzzle(self)),
        }
    }

    pub fn x_size(&self) -> usize {
        self.grid.len()
    }

    pub fn y_size(&self) -> usize {
        self.grid.first().unwrap().len()
    }

    pub fn count_contiguous(&self, x: usize, y: usize) -> (usize, usize, usize, usize) {
        let target_color = self.grid[x][y];

        let mut up = 0;
        for yi in (0..y).rev() {
            if self.grid[x][yi] == target_color {
                up += 1;
            } else {
                break;
            }
        }

        let mut down = 0;
        for yi in (y + 1)..self.y_size() {
            if self.grid[x][yi] == target_color {
                down += 1;
            } else {
                break;
            }
        }

        let mut left = 0;
        for xi in (0..x).rev() {
            if self.grid[xi][y] == target_color {
                left += 1;
            } else {
                break;
            }
        }

        let mut right = 0;
        for xi in (x + 1)..self.x_size() {
            if self.grid[xi][y] == target_color {
                right += 1;
            } else {
                break;
            }
        }

        (up, down, left, right)
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
        _ => NonogramFormat::CharGrid,
    }
}

#[derive(Clone, Debug)]
pub struct Document {
    p: Option<DynPuzzle>,
    s: Option<Solution>,
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
            problems.push("warning: missing author".to_string());
        }

        if let Ok(solution) = self.solution() {
            problems.extend(solution.quality_check());
        }

        let puzzle = self.puzzle();
        match puzzle.plain_solve() {
            Ok(report) => {
                let mut unsolved_count = 0;
                for col in &report.solution.grid {
                    for cell in col {
                        if *cell == UNSOLVED {
                            unsolved_count += 1;
                        }
                    }
                }
                if unsolved_count > 0 {
                    problems.push(format!("Puzzle has {} unsolved cells", unsolved_count));
                }
            }
            Err(_) => {
                problems.push("Puzzle is unsolvable".to_string());
            }
        }

        problems
    }

    pub fn new(
        puzzle: Option<DynPuzzle>,
        solution: Option<Solution>,
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
            for row in &solution.grid {
                for color in row {
                    color.hash(&mut hasher);
                }
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

    // TODO: this is just for debugging
    pub fn any_unsolved(&self) -> bool {
        if let Some(solution) = &self.s {
            for line in &solution.grid {
                for &color in line {
                    if color == UNSOLVED {
                        return true;
                    }
                }
            }
        }
        return false;
    }

    pub fn dimensions(&self) -> (usize, usize) {
        if let Some(solution) = &self.s {
            (solution.x_size(), solution.y_size())
        } else {
            self.p.as_ref().unwrap().specialize(
                |n| (n.cols.len(), n.rows.len()),
                |t| (t.cols.len(), t.rows.len()),
            )
        }
    }

    pub fn puzzle(&mut self) -> &DynPuzzle {
        if self.p.is_none() {
            self.p = Some(self.s.as_ref().unwrap().to_puzzle());
        }
        self.p.as_ref().unwrap()
    }

    pub fn try_solution(&self) -> Option<&Solution> {
        self.s.as_ref()
    }

    pub fn solution(&mut self) -> anyhow::Result<&Solution> {
        if self.s.is_none() {
            self.s = Some(self.p.as_ref().unwrap().plain_solve()?.solution)
        }
        Ok(self.s.as_ref().unwrap())
    }

    pub fn solution_mut(&mut self) -> &mut Solution {
        if self.s.is_none() {
            self.s = Some(self.p.as_ref().unwrap().plain_solve().unwrap().solution)
        }
        self.p = None; // Edits will invalidate the puzzle!
        self.s.as_mut().unwrap()
    }

    pub fn take_solution(self) -> anyhow::Result<Solution> {
        match self.s {
            Some(s) => Ok(s),
            None => self.p.unwrap().plain_solve().map(|r| r.solution),
        }
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

    pub fn from_solution(solution: Solution, file: String) -> Self {
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
