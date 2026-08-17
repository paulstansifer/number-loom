use crate::geometry::{GridKind, Shape, Square, Tri};
use crate::puzzle::{ClueStyle, Color, ColorInfo, Document, DynSolution, Solution};
use base64::{Engine as _, engine::general_purpose};
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use std::io::prelude::*;

#[derive(Serialize, Deserialize, PartialEq, Debug)]
pub struct WovenVersion0 {
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub title: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub description: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub author: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    pub solution: SerializableSolution,
}

/// If we ever have to break backwards-compatibility,
/// we can create a new version here.
#[derive(Serialize, Deserialize, PartialEq, Debug)]
pub enum WovenDocument {
    V0(WovenVersion0),
}

#[derive(Serialize, Deserialize, PartialEq, Debug)]
pub struct SerializableSolution {
    pub clue_style: ClueStyle,
    pub palette: Vec<ColorInfo>,
    /// Square dimensions, or a triddler outline.
    pub shape: Shape,
    /// One color per cell, in the dense order the shape implies.
    pub cells: Vec<Color>,
}

impl From<&mut Document> for WovenVersion0 {
    fn from(doc: &mut Document) -> Self {
        WovenVersion0 {
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

/// The share string for an already-serializable document. Split out from `to_woven` so that
/// `golden_tests` can encode a `WovenDocument` it read from a file, rather than one that has
/// been round-tripped through a `Document` and normalized on the way.
fn encode_woven(s_doc: &WovenDocument) -> anyhow::Result<String> {
    let buf = std::io::BufWriter::new(Vec::new());
    let mut encoder = brotli::CompressorWriter::new(buf, 4096, 11, 22);
    let bytes = serde_json::to_vec(s_doc)?;

    encoder.write_all(&bytes)?;
    let compressed = encoder.into_inner().into_inner().unwrap();
    let encoded = format!(
        "WOVEN-{}-",
        general_purpose::STANDARD_NO_PAD.encode(compressed) // base-64
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

/// The counterpart to `encode_woven`: everything `from_woven` does except the last step of
/// turning the format back into a `Document`.
fn decode_woven(s: &str) -> anyhow::Result<WovenDocument> {
    let s = s
        .strip_prefix("WOVEN-")
        .ok_or_else(|| anyhow::anyhow!("Missing 'WOVEN-' prefix"))?
        .strip_suffix("-")
        .ok_or_else(|| anyhow::anyhow!("Must end in a '-'"))?;
    let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    let compressed = general_purpose::STANDARD_NO_PAD.decode(s.as_bytes())?; // base-64

    let mut decoder = brotli::Decompressor::new(&compressed[..], 4096);
    let mut bytes = Vec::new();
    decoder.read_to_end(&mut bytes)?;

    Ok(serde_json::from_slice(&bytes)?)
}

pub fn to_woven(doc: &mut Document) -> anyhow::Result<String> {
    encode_woven(&WovenDocument::V0(doc.into()))
}

pub fn from_woven(s: &str, filename: String) -> anyhow::Result<Document> {
    let mut doc: Document = match decode_woven(s)? {
        WovenDocument::V0(s_doc_v0) => s_doc_v0.into(),
    };

    doc.file = filename;
    Ok(doc)
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

        let s_doc: WovenVersion0 = (&mut doc).into();
        let mut new_doc: Document = s_doc.into();

        // .file is lost, which is fine
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

        let s_doc: WovenVersion0 = (&mut doc).into();
        let mut new_doc: Document = s_doc.into();

        // .file is lost, which is fine.
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
        // The filename is lost, but that's okay!
        let mut new_doc = from_woven(&share_string, "test.webpbn".to_string()).unwrap();

        assert_eq!(doc.file, new_doc.file);
        assert_eq!(doc.title, new_doc.title);
        assert_eq!(doc.description, new_doc.description);
        assert_eq!(doc.author, new_doc.author);
        assert_eq!(doc.id, new_doc.id);
        assert_eq!(doc.license, new_doc.license);
        assert_eq!(doc.puzzle(), new_doc.puzzle());
    }
}

impl From<WovenVersion0> for Document {
    fn from(s_doc: WovenVersion0) -> Self {
        Document::new(
            None,
            Some((&s_doc.solution).into()),
            "".to_string(),
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
            palette: solution.palette.values().cloned().sorted().collect(),
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

/// Frozen share strings, plus the document they all mean.
///
/// Each fixture in `examples/woven` is a group of files: one `<name>.<n>.woven` per encoding the
/// program has ever produced for that document, and one `<name>.json` holding that document
/// spelled out for human reading. Two claims tie them together:
///
/// 1. every `.woven` in a group, and the `.json`, mean the same document;
/// 2. what the current code writes is byte-for-byte one of the `.woven` files.
///
/// Together those say what backwards compatibility means here. (1) is the reading half: a change
/// that makes an old share string decode differently — or not at all — breaks it, and no amount
/// of regenerating can hide that, because the old file stays. (2) is the writing half: a change
/// to what we emit is allowed, but it has to be noticed and recorded rather than discovered by a
/// user whose link stopped working.
///
/// That's why the update path is safe to run: it only ever rewrites the `.json`, which is
/// derived, and *adds* a numbered `.woven`, which is a new claim rather than a retraction of an
/// old one. No `.woven` file is ever overwritten or deleted — each is evidence about a version
/// that shipped, and deleting it deletes the only record that the format used to look that way.
///
/// The one place to be careful is a group with a single `.woven`, where (1) has nothing to
/// compare against and the `.json` diff is the only sign that a share string's meaning moved.
/// Groups grow that second file the first time the encoding changes, and are stronger after.
#[cfg(test)]
mod golden_tests {
    use super::*;
    use crate::geometry::{Geometry, Outline, Rect};
    use crate::puzzle::{BACKGROUND, Corner, UNSOLVED};
    use std::collections::{BTreeMap, HashMap};
    use std::path::{Path, PathBuf};

    const GOLDEN_DIR: &str = "examples/woven";

    /// Set this to record a change to what the current code writes: it refreshes every `.json`
    /// and adds a `.woven` for any encoding not already on file.
    const UPDATE_VAR: &str = "UPDATE_WOVEN_SNAPSHOTS";

    fn updating() -> bool {
        std::env::var(UPDATE_VAR).is_ok()
    }

    fn color(
        ch: char,
        name: &str,
        rgb: (u8, u8, u8),
        color: Color,
        corner: Option<Corner>,
    ) -> ColorInfo {
        ColorInfo {
            ch,
            name: name.to_string(),
            rgb,
            color,
            corner,
        }
    }

    fn palette(entries: Vec<ColorInfo>) -> HashMap<Color, ColorInfo> {
        entries.into_iter().map(|ci| (ci.color, ci)).collect()
    }

    /// Cells drawn as a picture, one string per row, using each color's `ch`. A fixture is only
    /// useful if a reader can see what it contains, and twenty bare `Color(2)`s are not that.
    fn picture(palette: &HashMap<Color, ColorInfo>, rows: &[&str]) -> Vec<Color> {
        rows.iter()
            .flat_map(|row| row.chars())
            .map(|ch| {
                palette
                    .values()
                    .find(|ci| ci.ch == ch)
                    .unwrap_or_else(|| panic!("no palette entry draws {ch:?}"))
                    .color
            })
            .collect()
    }

    /// The documents the fixtures were first generated from. Between them they must cover every
    /// variant and field the format can carry, since a type that no fixture exercises is a type
    /// that can break compatibility without any test noticing: both `Shape` variants, both
    /// `ClueStyle` variants, `Corner` in all four orientations and absent, `UNSOLVED` cells,
    /// non-ASCII palette characters, and the optional metadata both present and missing.
    fn corpus() -> Vec<(&'static str, Document)> {
        let mut fixtures = Vec::new();

        // The plainest thing the format can hold: black and white, square, no metadata.
        let bw = crate::import::bw_palette();
        let cells = picture(&bw, &[" ## ", "#  #", "####"]);
        fixtures.push((
            "square_bw",
            Document::from_solution(
                DynSolution::Square(Solution::new(
                    ClueStyle::Nono,
                    bw,
                    Geometry::<Square>::new(Rect {
                        width: 4,
                        height: 3,
                    }),
                    cells,
                )),
                "square_bw.woven".to_string(),
            ),
        ));

        // Every metadata field filled in, a multicolor palette, and a non-ASCII `ch` — `ColorInfo`
        // stores a `char`, so this pins down that it survives as a character and not a byte.
        let colorful = palette(vec![
            color(' ', "white", (255, 255, 255), BACKGROUND, None),
            color('#', "black", (0, 0, 0), Color(1), None),
            color('▓', "crimson", (220, 20, 60), Color(2), None),
            color('~', "sky", (135, 206, 235), Color(3), None),
        ]);
        let cells = picture(&colorful, &["▓▓ ~~", "▓ # ~", " ### ", "~ # ▓"]);
        fixtures.push((
            "square_color_metadata",
            Document::new(
                None,
                Some(DynSolution::Square(Solution::new(
                    ClueStyle::Nono,
                    colorful,
                    Geometry::<Square>::new(Rect {
                        width: 5,
                        height: 4,
                    }),
                    cells,
                ))),
                "square_color_metadata.woven".to_string(),
                Some("Test Pattern".to_string()),
                Some("Every metadata field, populated.".to_string()),
                Some("Claude".to_string()),
                Some("golden-2".to_string()),
                Some("CC0".to_string()),
            ),
        ));

        // Trianogram clues, which is the only thing that puts a `Corner` in a palette. All four
        // orientations appear, so a change to the meaning of `upper`/`left` cannot slip through.
        let triano = palette(vec![
            color(' ', "white", (255, 255, 255), BACKGROUND, None),
            color('#', "black", (0, 0, 0), Color(1), None),
            color(
                '`',
                "upper left",
                (0, 0, 0),
                Color(2),
                Some(Corner {
                    upper: true,
                    left: true,
                }),
            ),
            color(
                '\'',
                "upper right",
                (0, 0, 0),
                Color(3),
                Some(Corner {
                    upper: true,
                    left: false,
                }),
            ),
            color(
                ',',
                "lower left",
                (0, 0, 0),
                Color(4),
                Some(Corner {
                    upper: false,
                    left: true,
                }),
            ),
            color(
                '.',
                "lower right",
                (0, 0, 0),
                Color(5),
                Some(Corner {
                    upper: false,
                    left: false,
                }),
            ),
        ]);
        let cells = picture(&triano, &["'##`", "####", ".##,"]);
        fixtures.push((
            "square_triano",
            Document::from_solution(
                DynSolution::Square(Solution::new(
                    ClueStyle::Triano,
                    triano,
                    Geometry::<Square>::new(Rect {
                        width: 4,
                        height: 3,
                    }),
                    cells,
                )),
                "square_triano.woven".to_string(),
            ),
        ));

        // A triddler, mid-edit: an `Outline` that isn't a regular hexagon (so the six bounds are
        // all distinguishable) and cells still set to `UNSOLVED`. A triddler's cells don't form
        // rows a picture could show, so this one is built by index.
        let geometry = Geometry::<Tri>::new(Outline {
            a: (0, 3),
            b: (0, 2),
            c: (-1, 1),
        });
        let mut tri_palette = crate::import::bw_palette();
        tri_palette.insert(
            UNSOLVED,
            color('?', "unsolved", (128, 128, 128), UNSOLVED, None),
        );
        let cells: Vec<Color> = (0..geometry.cell_count())
            .map(|i| match i % 3 {
                0 => BACKGROUND,
                1 => Color(1),
                _ => UNSOLVED,
            })
            .collect();
        fixtures.push((
            "triddler_partial",
            Document::from_solution(
                DynSolution::Tri(Solution::new(ClueStyle::Nono, tri_palette, geometry, cells)),
                "triddler_partial.woven".to_string(),
            ),
        ));

        fixtures
    }

    /// One fixture: the `.json`, and every `.woven` recorded for it, oldest first.
    struct Fixture {
        name: String,
        json: PathBuf,
        wovens: Vec<(u32, PathBuf)>,
    }

    impl Fixture {
        /// The number to give the next `.woven` file added to this group.
        fn next_number(&self) -> u32 {
            self.wovens.last().map_or(0, |(n, _)| n + 1)
        }

        fn woven_path(&self, number: u32) -> PathBuf {
            PathBuf::from(GOLDEN_DIR).join(format!("{}.{number}.woven", self.name))
        }
    }

    /// Groups `examples/woven` by fixture name. `<name>.<n>.woven` is the only shape a share
    /// string file may have, so that a stray or misnamed file is a failure rather than a fixture
    /// that silently stops being checked.
    fn fixtures() -> Vec<Fixture> {
        let mut groups: BTreeMap<String, Vec<(u32, PathBuf)>> = BTreeMap::new();

        for entry in std::fs::read_dir(GOLDEN_DIR)
            .unwrap_or_else(|e| panic!("cannot read {GOLDEN_DIR}: {e}"))
        {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("woven") {
                continue;
            }
            let stem = path.file_stem().unwrap().to_str().unwrap();
            let (name, number) = stem.split_once('.').unwrap_or_else(|| {
                panic!(
                    "{}: share strings must be named <fixture>.<number>.woven",
                    path.display()
                )
            });
            let number: u32 = number.parse().unwrap_or_else(|_| {
                panic!("{}: {number:?} is not a version number", path.display())
            });
            groups
                .entry(name.to_string())
                .or_default()
                .push((number, path));
        }

        assert!(!groups.is_empty(), "no fixtures found in {GOLDEN_DIR}");

        groups
            .into_iter()
            .map(|(name, mut wovens)| {
                wovens.sort();
                Fixture {
                    json: PathBuf::from(GOLDEN_DIR).join(format!("{name}.json")),
                    name,
                    wovens,
                }
            })
            .collect()
    }

    /// The format as text. Every comparison below goes through this rather than through
    /// `PartialEq`, so that a failure can be looked at line by line.
    fn as_json(s_doc: &WovenDocument) -> String {
        serde_json::to_string_pretty(s_doc).unwrap() + "\n"
    }

    /// Where a failure leaves a copy of something that isn't on disk anywhere else.
    const SCRATCH_DIR: &str = "/tmp";

    /// Drops `text` in `/tmp/` and hands back the path, so that a mismatch can be reported as a
    /// `diff` command to run.
    fn scratch_copy(file_name: &str, text: &str) -> PathBuf {
        let path = PathBuf::from(SCRATCH_DIR).join(file_name);
        std::fs::write(&path, text).unwrap_or_else(|e| panic!("cannot write {path:?}: {e}"));
        path
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
    }

    /// What the current code makes of a stored document: loaded the way the program loads it,
    /// then written back out as text.
    ///
    /// The comparisons below all happen here rather than on the raw deserialized value, because
    /// two files can spell the same document differently — a palette listed in another order is
    /// the obvious case — and that is not a compatibility break. Normalizing through the current
    /// reader and writer asks the question that matters: does this old file still *mean* what the
    /// others mean? It also keeps the update path usable, since recording a new encoding must not
    /// put the fixture into a state where the old and new files can never agree again.
    fn canonical_form(s_doc: WovenDocument, source: &Path) -> String {
        let mut doc: Document = match s_doc {
            WovenDocument::V0(v0) => v0.into(),
        };
        let _ = doc
            .solution()
            .unwrap_or_else(|e| panic!("{}: has no usable solution: {e:?}", source.display()));
        as_json(&WovenDocument::V0((&mut doc).into()))
    }

    fn canonical_form_of_share_string(path: &Path) -> String {
        let text = read(path);
        let s_doc = decode_woven(&text)
            .unwrap_or_else(|e| panic!("{}: no longer decodes: {e:?}", path.display()));
        canonical_form(s_doc, path)
    }

    /// Loads a fixture and saves it again, by exactly the route the program itself takes.
    ///
    /// Going the short way — decoding to a `WovenDocument` and re-encoding that — would test
    /// almost nothing, because the palette is already a sorted `Vec` by then. Everything worth
    /// watching happens in the conversions on either side of `Solution`, so the trip has to be
    /// the real one.
    fn save_as_the_program_would(path: &Path) -> String {
        let mut doc = from_woven(&read(path), path.to_string_lossy().to_string())
            .unwrap_or_else(|e| panic!("{}: no longer loads: {e:?}", path.display()));
        to_woven(&mut doc)
            .unwrap_or_else(|e| panic!("{}: cannot be saved again: {e:?}", path.display()))
    }

    /// The two claims that make up backwards compatibility here, checked together because in
    /// update mode both of them write into `examples/woven`, and two tests doing that at once
    /// would race over the directory they are both reading.
    #[test]
    fn share_strings_stay_compatible() {
        for fixture in fixtures() {
            let (oldest_number, oldest_path) = &fixture.wovens[0];
            let expected = canonical_form_of_share_string(oldest_path);

            // Claim (1): every recorded encoding, and the `.json`, mean the same document. This
            // is the half that cannot be updated away — a failure here means a share string that
            // some version of this program handed to a user no longer says what it used to.
            for (number, path) in &fixture.wovens[1..] {
                let actual = canonical_form_of_share_string(path);
                if expected == actual {
                    continue;
                }
                let name = &fixture.name;
                panic!(
                    "{}: encoding {number} of {name} no longer means the same as encoding \
                     {oldest_number}. To see how they differ:\n    diff {} {}\n\
                     These are all supposed to be the same document, so the format has changed \
                     meaning and not merely appearance. Recording a new encoding will not fix \
                     this; the change needs to be undone, or made under a new `WovenDocument` \
                     version.",
                    path.display(),
                    scratch_copy(&format!("{name}.{oldest_number}.json"), &expected).display(),
                    scratch_copy(&format!("{name}.{number}.json"), &actual).display(),
                );
            }

            // The `.json` is derived from the share strings, so update mode just rewrites it.
            // It is compared as text rather than by meaning, unlike the files above: its job is
            // to be what the current writer emits, laid out for reading, so a change in how the
            // palette is ordered is exactly the sort of thing it exists to show.
            //
            // With a single recorded encoding the loop above has nothing to compare, so this
            // text is the only thing standing between a change in what a share string *means*
            // and a silent update. Update mode therefore keeps the version it replaced, and the
            // rewritten file still has to survive review as part of the commit.
            if updating() {
                // A missing `.json` is not an error to update mode: it is derived, so it can
                // always be rebuilt from the share strings.
                let previous = std::fs::read_to_string(&fixture.json).ok();
                std::fs::write(&fixture.json, &expected).unwrap();
                match previous {
                    None => println!("{}: created", fixture.json.display()),
                    Some(previous) if previous != expected => println!(
                        "{}: recorded a change to what we write. To see it:\n    diff {} {}",
                        fixture.json.display(),
                        scratch_copy(&format!("{}.json.was", fixture.name), &previous).display(),
                        fixture.json.display(),
                    ),
                    Some(_) => {}
                }
            } else {
                assert!(
                    fixture.json.exists(),
                    "{} is missing. It is derived from the share strings, so it can be rebuilt:\
                     \n    {UPDATE_VAR}=1 cargo test --lib golden -- --nocapture",
                    fixture.json.display(),
                );
                let actual = read(&fixture.json);
                assert!(
                    expected == actual,
                    "{} is not what we would write for {}. To see the difference:\n    \
                     diff {} {}\nIf the share strings are right, refresh it with the command:\n    \
                     {UPDATE_VAR}=1 cargo test --lib golden -- --nocapture",
                    fixture.json.display(),
                    oldest_path.display(),
                    fixture.json.display(),
                    scratch_copy(&format!("{}.json", fixture.name), &expected).display(),
                );
            }

            // Claim (2): what we write today is one of the encodings on file. Emitting something
            // new is allowed — the format may change how it spells things — but it has to be
            // written down, so that later versions keep being held to reading it.
            let current = save_as_the_program_would(oldest_path);
            if fixture
                .wovens
                .iter()
                .any(|(_, path)| read(path).trim_end() == current.trim_end())
            {
                continue;
            }

            let next = fixture.woven_path(fixture.next_number());
            assert!(
                updating(),
                "{} now encodes to something not on file. That is fine if older versions can \
                 still read it — check the diff in {} first — but it has to be recorded: re-run \
                 with {UPDATE_VAR}=1, which will write {}. If older versions *cannot* read it, \
                 the change needs a new `WovenDocument` version instead.",
                fixture.name,
                fixture.json.display(),
                next.display(),
            );
            std::fs::write(&next, &current).unwrap();
            println!("recorded a new encoding: {}", next.display());
        }
    }

    /// Palette order has to depend only on the palette's contents. A `Solution` holds its palette
    /// in a `HashMap`, whose iteration order differs from run to run, so anything that leaks that
    /// order through makes the same document encode differently every time it is saved.
    #[test]
    fn encoding_does_not_depend_on_hash_order() {
        for fixture in fixtures() {
            // Each load builds a fresh `HashMap`, so a few rounds is enough to shake out an
            // order that isn't fully determined by the palette itself.
            let path = &fixture.wovens[0].1;
            let first = save_as_the_program_would(path);
            for _ in 0..8 {
                assert_eq!(
                    first,
                    save_as_the_program_would(path),
                    "{}: saving the same document twice produced two different share strings",
                    fixture.name,
                );
            }
        }
    }

    /// A fixture that was added to `corpus` but never generated would silently test nothing.
    #[test]
    fn every_corpus_entry_has_a_fixture() {
        let names: Vec<String> = fixtures().into_iter().map(|f| f.name).collect();
        for (name, _) in corpus() {
            assert!(
                names.iter().any(|n| n == name),
                "{name} has no files in {GOLDEN_DIR}; run \
                 `cargo test --lib generate_missing_fixtures -- --ignored`",
            );
        }
    }

    /// Writes `<name>.0.woven` and `<name>.json` for any fixture in `corpus` with no files yet.
    ///
    /// Ignored by default, and it will not replace a share string that already exists — changing
    /// what a fixture *is* means throwing the whole group away and starting over, which has to be
    /// a deliberate `rm`, not something a test does because `corpus` was edited.
    #[test]
    #[ignore = "run by hand when adding a fixture; writes into examples/woven"]
    fn generate_missing_fixtures() {
        let existing = fixtures();
        let dir = PathBuf::from(GOLDEN_DIR);
        for (name, mut doc) in corpus() {
            let Some(fixture) = existing.iter().find(|f| f.name == name) else {
                let s_doc = WovenDocument::V0((&mut doc).into());
                std::fs::write(
                    dir.join(format!("{name}.0.woven")),
                    to_woven(&mut doc).unwrap(),
                )
                .unwrap();
                std::fs::write(dir.join(format!("{name}.json")), as_json(&s_doc)).unwrap();
                println!("wrote {name}.0.woven and {name}.json");
                continue;
            };

            // The `.json` is derived, so a group that has lost only that one can be repaired
            // from the share strings it still has.
            if !fixture.json.exists() {
                let rebuilt = canonical_form_of_share_string(&fixture.wovens[0].1);
                std::fs::write(&fixture.json, rebuilt).unwrap();
                println!(
                    "rebuilt {} from {}",
                    fixture.json.display(),
                    fixture.wovens[0].1.display()
                );
                continue;
            }

            println!(
                "{name} already has {} share string(s) and a .json, so nothing was written. \
                 Editing its entry in `corpus` does not change them: the share strings are \
                 records of what shipped, not outputs. To rebuild this fixture from `corpus` \
                 anyway — which is only right if it has never been committed — remove the whole \
                 group first:\n    rm {}/{name}.*\n",
                fixture.wovens.len(),
                GOLDEN_DIR,
            );
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
        let mut reloaded = from_woven(&share_string, "wip.woven".to_string()).unwrap();
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
        let mut reloaded =
            from_woven(&to_woven(&mut doc).unwrap(), "sq.woven".to_string()).unwrap();
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
