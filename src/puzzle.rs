use std::collections::HashMap;
use std::fmt::Debug;
use std::hash::Hash;

use crate::import::{solution_to_puzzle, solution_to_triano_puzzle};
pub trait Clue: Clone + Copy + Debug + PartialEq + Eq + Hash {
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

#[derive(PartialEq, Eq, Clone, Copy, Hash)]
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

#[derive(PartialEq, Eq, Clone, Copy, Hash)]
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

#[derive(PartialEq, Eq, Clone, Copy, Hash, Debug, PartialOrd, Ord)]
pub struct Color(pub u8);

pub static BACKGROUND: Color = Color(0);
pub static UNSOLVED: Color = Color(255);

// A triangle-shaped half of a square. `true` means solid in the given direction.
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub struct Corner {
    pub upper: bool,
    pub left: bool,
}

// Note that `rgb` is not necessarily unique!
// But `ch` and `name` ought to be, along with `rgb` + `corner`.
#[derive(PartialEq, Eq, Clone, Debug)]
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

#[derive(Clone, Debug)]
pub struct Solution {
    pub clue_style: ClueStyle,
    pub palette: HashMap<Color, ColorInfo>, // should include the background!
    pub grid: Vec<Vec<Color>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Puzzle<C: Clue> {
    pub palette: HashMap<Color, ColorInfo>, // should include the background!
    pub rows: Vec<Vec<C>>,
    pub cols: Vec<Vec<C>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DynPuzzle {
    Nono(Puzzle<Nono>),
    Triano(Puzzle<Triano>),
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
        match p {
            DynPuzzle::Nono(puzzle) => {
                crate::grid_solve::solve(puzzle, &mut self.nono_cache, false)
            }
            DynPuzzle::Triano(puzzle) => {
                crate::grid_solve::solve(puzzle, &mut self.triano_cache, false)
            }
        }
    }
}

impl DynPuzzle {
    pub fn plain_solve(&self) -> anyhow::Result<crate::grid_solve::Report> {
        match self {
            DynPuzzle::Nono(puzzle) => crate::grid_solve::solve(puzzle, &mut None, false),
            DynPuzzle::Triano(puzzle) => crate::grid_solve::solve(puzzle, &mut None, false),
        }
    }

    pub fn solve_with_args(&self, trace_solve: bool) -> anyhow::Result<crate::grid_solve::Report> {
        match self {
            DynPuzzle::Nono(puzzle) => crate::grid_solve::solve(puzzle, &mut None, trace_solve),
            DynPuzzle::Triano(puzzle) => crate::grid_solve::solve(puzzle, &mut None, trace_solve),
        }
    }

    pub fn specialize<FN, FT, T>(&self, f_n: FN, f_t: FT) -> T
    where
        FN: FnOnce(&Puzzle<Nono>) -> T,
        FT: FnOnce(&Puzzle<Triano>) -> T,
    {
        match self {
            DynPuzzle::Nono(puzzle) => f_n(puzzle),
            DynPuzzle::Triano(puzzle) => f_t(puzzle),
        }
    }

    pub fn assume_nono(&self) -> &Puzzle<Nono> {
        match self {
            DynPuzzle::Nono(puzzle) => puzzle,
            DynPuzzle::Triano(_) => panic!("must be a true nonogram!"),
        }
    }

    #[cfg(test)] // Until needed normally
    pub fn assume_triano(&self) -> &Puzzle<Triano> {
        match self {
            DynPuzzle::Triano(puzzle) => puzzle,
            DynPuzzle::Nono(_) => panic!("must be a trianogram!"),
        }
    }
}

impl Solution {
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

#[derive(Clone, Copy, Debug, clap::ValueEnum, Default, PartialEq, Eq)]
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

pub struct Document {
    p: Option<DynPuzzle>,
    s: Option<Solution>,
    /// Path if native, just a filename, if on the Web
    file: String,
}

impl Document {
    pub fn new(puzzle: Option<DynPuzzle>, solution: Option<Solution>, file: String) -> Document {
        assert!(puzzle.is_some() || solution.is_some());
        Document {
            p: puzzle,
            s: solution,
            file,
        }
    }

    #[allow(dead_code)] // it's a little weird how this is easy to get but never used
    pub fn file(&self) -> &str {
        &self.file
    }

    #[allow(dead_code)]
    pub fn try_puzzle(&self) -> Option<&DynPuzzle> {
        self.p.as_ref()
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

    pub fn take_solution(self) -> anyhow::Result<Solution> {
        match self.s {
            Some(s) => Ok(s),
            None => self.p.unwrap().plain_solve().map(|r| r.solution),
        }
    }
}
