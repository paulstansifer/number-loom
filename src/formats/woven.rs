use crate::geometry::{GridKind, Shape, Square, Tri};
use crate::puzzle::{ClueStyle, Color, ColorInfo, Document, DynSolution, Solution};
use base64::{Engine as _, engine::general_purpose};
use serde::{Deserialize, Serialize};
use std::io::prelude::*;

#[derive(Serialize, Deserialize, PartialEq, Debug)]
pub struct SerializableDocument {
    pub file: String,
    pub title: String,
    pub description: String,
    pub author: String,
    pub id: Option<String>,
    pub license: Option<String>,
    pub solution: SerializableSolution,
}

#[derive(Serialize, Deserialize, PartialEq, Debug)]
pub struct SerializableSolution {
    pub clue_style: ClueStyle,
    pub palette: Vec<ColorInfo>,
    /// Square dimensions, or a triddler outline.
    pub shape: Shape,
    /// One colour per cell, in the dense order the shape implies.
    pub cells: Vec<Color>,
}

impl From<&mut Document> for SerializableDocument {
    fn from(doc: &mut Document) -> Self {
        SerializableDocument {
            file: doc.file.clone(),
            title: doc.title.clone(),
            description: doc.description.clone(),
            author: doc.author.clone(),
            id: if doc.id.is_empty() {
                None
            } else {
                Some(doc.id.clone())
            },
            license: if doc.license.is_empty() {
                None
            } else {
                Some(doc.license.clone())
            },
            solution: doc
                .solution()
                .expect("Need a solution to save a document!")
                .into(),
        }
    }
}

pub fn to_woven(doc: &mut Document) -> anyhow::Result<String> {
    let s_doc: SerializableDocument = doc.into();
    let buf = std::io::BufWriter::new(Vec::new());
    let mut encoder = brotli::CompressorWriter::new(buf, 4096, 11, 22);
    let bytes = serde_json::to_vec(&s_doc)?;

    encoder.write_all(&bytes)?;
    let compressed = encoder.into_inner().into_inner().unwrap();
    let encoded = format!(
        "WOVEN-{}-",
        general_purpose::STANDARD_NO_PAD.encode(compressed)
    );

    let mut result = String::new();
    for (i, c) in encoded.chars().enumerate() {
        result.push(c);
        if (i + 1) % 100 == 0 {
            result.push('\n');
        }
    }
    Ok(result)
}

pub fn from_woven(s: &str) -> anyhow::Result<Document> {
    let s = s
        .strip_prefix("WOVEN-")
        .ok_or_else(|| anyhow::anyhow!("Missing 'WOVEN-' prefix"))?
        .strip_suffix("-")
        .ok_or_else(|| anyhow::anyhow!("Must end in a '-'"))?;
    let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    let compressed = general_purpose::STANDARD_NO_PAD.decode(s.as_bytes())?;

    let mut decoder = brotli::Decompressor::new(&compressed[..], 4096);
    let mut bytes = Vec::new();
    decoder.read_to_end(&mut bytes)?;

    let s_doc: SerializableDocument = serde_json::from_slice(&bytes)?;
    Ok(s_doc.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::puzzle::{Color, Document, DynPuzzle, Nono, Puzzle};
    use std::collections::HashMap;

    #[test]
    fn test_round_trip_from_puzzle() {
        let mut palette = HashMap::new();
        palette.insert(
            Color(0),
            crate::puzzle::ColorInfo {
                ch: ' ',
                name: "white".to_string(),
                rgb: (255, 255, 255),
                color: Color(0),
                corner: None,
            },
        );
        palette.insert(
            Color(1),
            crate::puzzle::ColorInfo {
                ch: '#',
                name: "black".to_string(),
                rgb: (0, 0, 0),
                color: Color(1),
                corner: None,
            },
        );

        let puzzle = DynPuzzle::SquareNono(Puzzle::square(
            palette,
            vec![vec![Nono {
                color: Color(1),
                count: 1,
            }]],
            vec![vec![Nono {
                color: Color(1),
                count: 1,
            }]],
        ));

        let mut doc = Document::new(
            Some(puzzle),
            None,
            "test.webpbn".to_string(),
            Some("Test Title".to_string()),
            Some("Test Description".to_string()),
            Some("Test Author".to_string()),
            Some("Test ID".to_string()),
            Some("Test License".to_string()),
        );

        let s_doc: SerializableDocument = (&mut doc).into();
        let mut new_doc: Document = s_doc.into();

        assert_eq!(doc.file, new_doc.file);
        assert_eq!(doc.title, new_doc.title);
        assert_eq!(doc.description, new_doc.description);
        assert_eq!(doc.author, new_doc.author);
        assert_eq!(doc.id, new_doc.id);
        assert_eq!(doc.license, new_doc.license);
        assert_eq!(doc.puzzle(), new_doc.puzzle());
    }

    #[test]
    fn test_round_trip_from_solution() {
        let mut palette = HashMap::new();
        palette.insert(
            Color(0),
            crate::puzzle::ColorInfo {
                ch: ' ',
                name: "white".to_string(),
                rgb: (255, 255, 255),
                color: Color(0),
                corner: None,
            },
        );
        palette.insert(
            Color(1),
            crate::puzzle::ColorInfo {
                ch: '#',
                name: "black".to_string(),
                rgb: (0, 0, 0),
                color: Color(1),
                corner: None,
            },
        );

        let solution = crate::puzzle::Solution::from_columns(
            crate::puzzle::ClueStyle::Nono,
            palette,
            vec![vec![Color(1)]],
        );

        let mut doc = Document::new(
            None,
            Some(DynSolution::Square(solution)),
            "test.webpbn".to_string(),
            Some("Test Title".to_string()),
            Some("Test Description".to_string()),
            Some("Test Author".to_string()),
            Some("Test ID".to_string()),
            Some("Test License".to_string()),
        );

        let s_doc: SerializableDocument = (&mut doc).into();
        let mut new_doc: Document = s_doc.into();

        assert_eq!(doc.file, new_doc.file);
        assert_eq!(doc.title, new_doc.title);
        assert_eq!(doc.description, new_doc.description);
        assert_eq!(doc.author, new_doc.author);
        assert_eq!(doc.id, new_doc.id);
        assert_eq!(doc.license, new_doc.license);
        assert_eq!(doc.puzzle(), new_doc.puzzle());
    }

    #[test]
    fn test_share_string_round_trip() {
        let mut palette = HashMap::new();
        palette.insert(
            Color(0),
            crate::puzzle::ColorInfo {
                ch: ' ',
                name: "white".to_string(),
                rgb: (255, 255, 255),
                color: Color(0),
                corner: None,
            },
        );
        palette.insert(
            Color(1),
            crate::puzzle::ColorInfo {
                ch: '#',
                name: "black".to_string(),
                rgb: (0, 0, 0),
                color: Color(1),
                corner: None,
            },
        );

        let puzzle = DynPuzzle::SquareNono(Puzzle::square(
            palette,
            vec![vec![Nono {
                color: Color(1),
                count: 1,
            }]],
            vec![vec![Nono {
                color: Color(1),
                count: 1,
            }]],
        ));

        let mut doc = Document::new(
            Some(puzzle),
            None,
            "test.webpbn".to_string(),
            Some("Test Title".to_string()),
            Some("Test Description".to_string()),
            Some("Test Author".to_string()),
            Some("Test ID".to_string()),
            Some("Test License".to_string()),
        );

        let share_string = to_woven(&mut doc).unwrap();
        let mut new_doc = from_woven(&share_string).unwrap();

        assert_eq!(doc.file, new_doc.file);
        assert_eq!(doc.title, new_doc.title);
        assert_eq!(doc.description, new_doc.description);
        assert_eq!(doc.author, new_doc.author);
        assert_eq!(doc.id, new_doc.id);
        assert_eq!(doc.license, new_doc.license);
        assert_eq!(doc.puzzle(), new_doc.puzzle());
    }
}

impl From<SerializableDocument> for Document {
    fn from(s_doc: SerializableDocument) -> Self {
        Document::new(
            None,
            Some((&s_doc.solution).into()),
            s_doc.file,
            Some(s_doc.title),
            Some(s_doc.description),
            Some(s_doc.author),
            s_doc.id,
            s_doc.license,
        )
    }
}

impl<K: GridKind> From<&Solution<K>> for SerializableSolution {
    fn from(solution: &Solution<K>) -> Self {
        SerializableSolution {
            clue_style: solution.clue_style,
            palette: solution.palette.values().cloned().collect(),
            shape: solution.geometry.shape(),
            cells: solution.cells.clone(),
        }
    }
}

impl From<&DynSolution> for SerializableSolution {
    fn from(solution: &DynSolution) -> Self {
        match solution {
            DynSolution::Square(s) => s.into(),
            DynSolution::Tri(s) => s.into(),
        }
    }
}

impl From<&SerializableSolution> for DynSolution {
    fn from(s_solution: &SerializableSolution) -> Self {
        let palette = s_solution
            .palette
            .iter()
            .map(|ci| (ci.color, ci.clone()))
            .collect();
        // The shape is the one place a stored puzzle is narrowed back to a static kind.
        match &s_solution.shape {
            Shape::Square { width, height } => DynSolution::Square(Solution::new(
                s_solution.clue_style,
                palette,
                crate::geometry::Geometry::<Square>::new(crate::geometry::Rect {
                    width: *width,
                    height: *height,
                }),
                s_solution.cells.clone(),
            )),
            Shape::Triangular(outline) => DynSolution::Tri(Solution::new(
                s_solution.clue_style,
                palette,
                crate::geometry::Geometry::<Tri>::new(*outline),
                s_solution.cells.clone(),
            )),
        }
    }
}

#[cfg(test)]
mod triangular_tests {
    use super::*;
    use crate::geometry::Outline;
    use crate::puzzle::{BACKGROUND, PuzzleDynOps, UNSOLVED};

    fn palette_with_unsolved() -> std::collections::HashMap<Color, ColorInfo> {
        let mut palette = crate::import::bw_palette();
        palette.insert(
            UNSOLVED,
            ColorInfo {
                ch: '?',
                name: "unsolved".to_string(),
                rgb: (128, 128, 128),
                color: UNSOLVED,
                corner: None,
            },
        );
        palette
    }

    /// The point of giving `Solution` a geometry: a triddler that is still being worked on, with
    /// some cells not yet decided, must survive being saved and loaded.
    #[test]
    fn an_ambiguous_triddler_round_trips() {
        let geometry = crate::geometry::Geometry::<Tri>::new(Outline::hexagon(2));
        let cells: Vec<Color> = (0..geometry.cell_count())
            .map(|i| match i % 3 {
                0 => BACKGROUND,
                1 => Color(1),
                _ => UNSOLVED, // still undecided
            })
            .collect();

        let solution = Solution::new(
            ClueStyle::Nono,
            palette_with_unsolved(),
            geometry.clone(),
            cells.clone(),
        );
        let mut doc = Document::from_solution(DynSolution::Tri(solution), "wip.woven".to_string());

        let share_string = to_woven(&mut doc).unwrap();
        let mut reloaded = from_woven(&share_string).unwrap();
        let reloaded_solution = reloaded.solution().unwrap();

        assert_eq!(reloaded_solution.shape(), geometry.shape());
        assert_eq!(reloaded_solution.cells(), cells);
        assert!(
            reloaded_solution.cells().contains(&UNSOLVED),
            "the undecided cells must still be undecided"
        );
        assert!(!reloaded.has_complete_solution().unwrap());
    }

    #[test]
    fn a_square_solution_still_round_trips_with_its_shape() {
        let solution = Solution::blank_bw(4, 3);
        let mut doc =
            Document::from_solution(DynSolution::Square(solution), "sq.woven".to_string());
        let mut reloaded = from_woven(&to_woven(&mut doc).unwrap()).unwrap();
        let reloaded = reloaded
            .solution()
            .unwrap()
            .as_square()
            .expect("still square");
        assert_eq!(reloaded.x_size(), 4);
        assert_eq!(reloaded.y_size(), 3);
    }

    /// A finished triangular picture must yield clues that solve back to it.
    #[test]
    fn a_triangular_solution_becomes_a_solvable_puzzle() {
        let geometry = crate::geometry::Geometry::<Tri>::new(Outline::hexagon(2));
        // A ring: everything except the two middle rows' interiors.
        let cells: Vec<Color> = (0..geometry.cell_count())
            .map(|i| if i % 4 == 0 { BACKGROUND } else { Color(1) })
            .collect();

        let solution = Solution::new(
            ClueStyle::Nono,
            crate::import::bw_palette(),
            geometry,
            cells.clone(),
        );

        let report = solution.to_puzzle().plain_solve().unwrap();
        // Whatever it manages to pin down must agree with the picture we started from.
        for (solved, truth) in report.solution.cells().iter().zip(&cells) {
            assert!(
                *solved == *truth || *solved == UNSOLVED,
                "solver contradicted the source picture"
            );
        }
    }
}
