use crate::puzzle::{ClueStyle, Color, ColorInfo, Document, DynPuzzle, Nono, Solution, Triano};
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
    pub grid: Vec<Vec<Color>>,
}

// Not currently used; doesn't work for ambiguous works-in-progress
#[derive(Serialize, Deserialize, PartialEq, Debug)]
pub enum SerializablePuzzle {
    Nono {
        palette: Vec<ColorInfo>,
        rows: Vec<Vec<Nono>>,
        cols: Vec<Vec<Nono>>,
    },
    Triano {
        palette: Vec<ColorInfo>,
        rows: Vec<Vec<Triano>>,
        cols: Vec<Vec<Triano>>,
    },
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

        let puzzle = DynPuzzle::Nono(Puzzle {
            palette,
            rows: vec![vec![Nono {
                color: Color(1),
                count: 1,
            }]],
            cols: vec![vec![Nono {
                color: Color(1),
                count: 1,
            }]],
        });

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

        let solution = crate::puzzle::Solution {
            clue_style: crate::puzzle::ClueStyle::Nono,
            palette,
            grid: vec![vec![Color(1)]],
        };

        let mut doc = Document::new(
            None,
            Some(solution),
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

        let puzzle = DynPuzzle::Nono(Puzzle {
            palette,
            rows: vec![vec![Nono {
                color: Color(1),
                count: 1,
            }]],
            cols: vec![vec![Nono {
                color: Color(1),
                count: 1,
            }]],
        });

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

impl From<&SerializablePuzzle> for DynPuzzle {
    fn from(s_puzzle: &SerializablePuzzle) -> Self {
        match s_puzzle {
            SerializablePuzzle::Nono {
                palette,
                rows,
                cols,
            } => DynPuzzle::Nono(crate::puzzle::Puzzle {
                palette: palette.iter().map(|ci| (ci.color, ci.clone())).collect(),
                rows: rows.clone(),
                cols: cols.clone(),
            }),
            SerializablePuzzle::Triano {
                palette,
                rows,
                cols,
            } => DynPuzzle::Triano(crate::puzzle::Puzzle {
                palette: palette.iter().map(|ci| (ci.color, ci.clone())).collect(),
                rows: rows.clone(),
                cols: cols.clone(),
            }),
        }
    }
}

impl From<&DynPuzzle> for SerializablePuzzle {
    fn from(puzzle: &DynPuzzle) -> Self {
        match puzzle {
            DynPuzzle::Nono(p) => SerializablePuzzle::Nono {
                palette: p.palette.values().cloned().collect(),
                rows: p.rows.clone(),
                cols: p.cols.clone(),
            },
            DynPuzzle::Triano(p) => SerializablePuzzle::Triano {
                palette: p.palette.values().cloned().collect(),
                rows: p.rows.clone(),
                cols: p.cols.clone(),
            },
        }
    }
}

impl From<&Solution> for SerializableSolution {
    fn from(solution: &Solution) -> Self {
        SerializableSolution {
            clue_style: solution.clue_style,
            palette: solution.palette.values().cloned().collect(),
            grid: solution.grid.clone(),
        }
    }
}

impl From<&SerializableSolution> for Solution {
    fn from(s_solution: &SerializableSolution) -> Self {
        Solution {
            clue_style: s_solution.clue_style,
            palette: s_solution
                .palette
                .iter()
                .map(|ci| (ci.color, ci.clone()))
                .collect(),
            grid: s_solution.grid.clone(),
        }
    }
}
