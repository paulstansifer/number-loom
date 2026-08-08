use anyhow::{Context, bail};
use std::collections::HashMap;

use crate::geometry::{ClueSet, ClueSetCounts, GridKind, Outline, Shape, Tri};
use crate::puzzle::{BACKGROUND, Color, ColorInfo, Document, DynPuzzle, Nono, Puzzle};

fn get_children<'a, 'input>(
    node: roxmltree::Node<'a, 'input>,
    tag: &str,
) -> anyhow::Result<Vec<roxmltree::Node<'a, 'input>>> {
    let mut res = vec![];

    for child in node.children() {
        if child.is_text() {
            if child.text().unwrap().trim() != "" {
                bail!("unexpected text: {}", child.text().unwrap());
            }
        }
        if child.is_element() {
            if child.tag_name().name() == tag {
                res.push(child);
            } else {
                bail!(
                    "unexpected element {}; was looking for {tag}",
                    child.tag_name().name()
                )
            }
        }
    }

    Ok(res)
}

fn get_single_child<'a, 'input>(
    node: roxmltree::Node<'a, 'input>,
    tag: &str,
) -> anyhow::Result<roxmltree::Node<'a, 'input>> {
    let mut res = get_children(node, tag)?;
    if res.len() == 0 {
        bail!("did not find the element {tag}");
    }
    if res.len() > 1 {
        bail!("expected only one element named {tag}");
    }
    Ok(res.pop().unwrap())
}

/// Assemble a triangular puzzle from webpbn's six clue sets.
///
/// The outline isn't stated anywhere in the file; it is implied by how many lines each set has.
fn triddler_puzzle(
    palette: HashMap<Color, ColorInfo>,
    clues: &HashMap<ClueSet, Vec<Vec<Nono>>>,
) -> anyhow::Result<Puzzle<Nono, Tri>> {
    let lines_in = |set: ClueSet| clues.get(&set).map(|v| v.len()).unwrap_or(0);
    let counts = ClueSetCounts {
        topleft: lines_in(ClueSet::TopLeft),
        bottomleft: lines_in(ClueSet::BottomLeft),
        top: lines_in(ClueSet::Top),
        topright: lines_in(ClueSet::TopRight),
        bottom: lines_in(ClueSet::Bottom),
        bottomright: lines_in(ClueSet::BottomRight),
    };

    let outline = Outline::from_clue_set_counts(counts)?;
    let geometry = crate::geometry::Geometry::<Tri>::new(outline);

    // Each set's lines are in increasing lane order, so they line up one-for-one with the lanes
    // the geometry assigns to that set.
    let mut lines = vec![vec![]; geometry.lane_map().lane_count()];
    for set in [
        ClueSet::TopLeft,
        ClueSet::BottomLeft,
        ClueSet::Top,
        ClueSet::TopRight,
        ClueSet::Bottom,
        ClueSet::BottomRight,
    ] {
        let Some(set_clues) = clues.get(&set) else {
            continue;
        };
        for (lane, clue_line) in geometry.lanes_in_clue_set(set).into_iter().zip(set_clues) {
            lines[lane] = clue_line.clone();
        }
    }

    Ok(Puzzle::triangular(palette, outline, lines))
}

pub fn webpbn_to_document(webpbn: &str) -> anyhow::Result<Document> {
    let doc = roxmltree::Document::parse(webpbn).context("could not parse XML")?;
    let puzzleset = doc.root_element();
    let puzzle_node = get_single_child(puzzleset, "puzzle")?;

    let mut title = None;
    let mut description = None;
    let mut author = None;
    let mut authorid = None;
    let mut id = None;
    let mut license = None;

    let default_color = puzzle_node
        .attribute("defaultcolor")
        .context("expected a 'defaultcolor' attribute")?;
    let mut next_color_index = 1;

    let mut named_colors = HashMap::<String, Color>::new();

    let mut palette = HashMap::<Color, ColorInfo>::new();
    let mut rows: Vec<Vec<Nono>> = vec![];
    let mut cols: Vec<Vec<Nono>> = vec![];
    // Triddlers split each of their three clue directions across two `<clues>` sets.
    let mut triddler_clues: HashMap<ClueSet, Vec<Vec<Nono>>> = HashMap::new();

    let triddler = match puzzle_node.attribute("type") {
        None | Some("grid") => false,
        Some("triddler") => true,
        Some(other) => bail!("unsupported puzzle type: {other}"),
    };

    for puzzle_part in puzzle_node.children() {
        if !puzzle_part.is_element() {
            continue;
        }

        let tag_name = puzzle_part.tag_name().name();
        if tag_name == "title" {
            title = puzzle_part.text().map(|s| s.trim().to_string());
        } else if tag_name == "description" {
            description = puzzle_part.text().map(|s| s.trim().to_string());
        } else if tag_name == "author" {
            author = puzzle_part.text().map(|s| s.trim().to_string());
        } else if tag_name == "authorid" {
            authorid = puzzle_part.text().map(|s| s.trim().to_string());
        } else if tag_name == "id" {
            id = puzzle_part.text().map(|s| s.trim().to_string());
        } else if tag_name == "copyright" {
            license = puzzle_part.text().map(|s| s.trim().to_string());
        } else if tag_name == "color" {
            let color_name = puzzle_part
                .attribute("name")
                .context("color element missing 'name' attribute")?;
            let color = if color_name == default_color {
                BACKGROUND
            } else {
                Color(next_color_index)
            };

            if color != BACKGROUND {
                next_color_index += 1
            }

            let hex_color = regex::Regex::new(
                r"^([0-9A-Za-z][0-9A-Za-z])([0-9A-Za-z][0-9A-Za-z])([0-9A-Za-z][0-9A-Za-z])$",
            )
            .unwrap();

            let color_text = puzzle_part.text().context("expected hex color in text")?;
            let (_, component_strs) = hex_color
                .captures(color_text)
                .context("expected a string of 6 hex digits")?
                .extract();

            let [r, g, b] = component_strs;
            let r = u8::from_str_radix(r, 16).context("expected hex digits")?;
            let g = u8::from_str_radix(g, 16).context("expected hex digits")?;
            let b = u8::from_str_radix(b, 16).context("expected hex digits")?;

            let color_info = ColorInfo {
                // TODO: error if there's more than one char!
                ch: puzzle_part
                    .attribute("char")
                    .context("color element missing 'char' attribute")?
                    .chars()
                    .next()
                    .context("'char' attribute is empty")?,
                name: color_name.to_string(),
                rgb: (r, g, b),
                color: color,
                corner: None, // webpbn isn't intended to represent Triano clues
            };

            palette.insert(color, color_info);
            named_colors.insert(color_name.to_string(), color);
        } else if tag_name == "clues" {
            let clue_type = puzzle_part.attribute("type").unwrap_or_default();
            let clue_set = match (triddler, clue_type) {
                (false, "rows") | (false, "columns") => None,
                (true, "topleft") => Some(ClueSet::TopLeft),
                (true, "bottomleft") => Some(ClueSet::BottomLeft),
                (true, "top") => Some(ClueSet::Top),
                (true, "topright") => Some(ClueSet::TopRight),
                (true, "bottom") => Some(ClueSet::Bottom),
                (true, "bottomright") => Some(ClueSet::BottomRight),
                (false, other) => {
                    bail!("expected clues of type 'rows' or 'columns', got '{other}'")
                }
                (true, other) => bail!("not a triddler clue direction: '{other}'"),
            };

            let mut clue_lanes = vec![];

            for lane in get_children(puzzle_part, "line")? {
                let mut clues = vec![];
                for block in get_children(lane, "count")? {
                    let color_name = block
                        .attribute("color")
                        .context("count element missing 'color' attribute")?;
                    let color = *named_colors
                        .get(color_name)
                        .with_context(|| format!("undefined color: {color_name}"))?;
                    let count_text = block.text().context("count element has no text")?;
                    let count: u16 = count_text
                        .parse()
                        .with_context(|| format!("expected a number, got: {count_text}"))?;
                    clues.push(Nono { color, count });
                }
                clue_lanes.push(clues);
            }

            match clue_set {
                Some(clue_set) => {
                    triddler_clues.insert(clue_set, clue_lanes);
                }
                None if clue_type == "rows" => rows = clue_lanes,
                None => cols = clue_lanes,
            }
        }
    }

    let puzzle: DynPuzzle = if triddler {
        triddler_puzzle(palette, &triddler_clues)?.into()
    } else {
        Puzzle::square(palette, rows, cols).into()
    };

    Ok(Document::new(
        Some(puzzle),
        None,
        "".to_string(),
        title,
        description,
        author.or(authorid),
        id,
        license,
    ))
}

/// webpbn describes `Nono` clues in either shape, so dispatch once and let the writer below be
/// generic over the grid kind.
pub fn as_webpbn(document: &Document) -> String {
    let mut document_with_puzzle = document.clone();
    match document_with_puzzle.puzzle() {
        DynPuzzle::SquareNono(p) => write_webpbn(document, p),
        DynPuzzle::TriNono(p) => write_webpbn(document, p),
        DynPuzzle::SquareTriano(_) => panic!("webpbn cannot represent trianogram clues"),
    }
}

fn write_webpbn<K: GridKind>(document: &Document, puzzle: &Puzzle<Nono, K>) -> String {
    use indoc::indoc;

    let palette = &puzzle.palette;

    let puzzle_type = match puzzle.geometry.shape() {
        Shape::Square { .. } => "grid",
        Shape::Triangular(_) => "triddler",
    };

    let mut res = String::new();
    // If you add <!DOCTYPE pbn SYSTEM "https://webpbn.com/pbn-0.3.dtd">, `pbnsolve` emits a warning.
    res.push_str(
        &indoc! {r#"
        <?xml version="1.0"?>
        <puzzleset>
        <puzzle type="{}" defaultcolor="white">
        <source>number-loom</source>
        "#}
        .replace("{}", puzzle_type),
    );
    if !document.title.is_empty() {
        res.push_str(&format!("<title>{}</title>\n", &document.title));
    }
    if !document.description.is_empty() {
        res.push_str(&format!(
            "<description>{}</description>\n",
            &document.description
        ));
    }
    if !document.author.is_empty() {
        res.push_str(&format!("<author>{}</author>\n", &document.author));
    }
    if !document.id.is_empty() {
        res.push_str(&format!("<id>{}</id>\n", &document.id));
    }
    if !document.license.is_empty() {
        res.push_str(&format!("<copyright>{}</copyright>\n", &document.license));
    }
    for color in palette.values() {
        let (r, g, b) = color.rgb;
        res.push_str(&format!(
            r#"<color name="{}" char="{}">{:02X}{:02X}{:02X}</color>"#,
            color.name, color.ch, r, g, b
        ));
        res.push('\n');
    }

    let write_clue_set = |res: &mut String, name: &str, lines: &[&Vec<Nono>]| {
        res.push_str(&format!(r#"<clues type="{name}">"#));
        for line in lines {
            res.push_str("<line>");
            for clue in line.iter() {
                res.push_str(&format!(
                    r#"<count color="{}">{}</count>"#,
                    palette[&clue.color].name, clue.count
                ));
            }
            res.push_str("</line>\n");
        }
        res.push_str(r#"</clues>"#);
        res.push('\n');
    };

    match puzzle.geometry.shape() {
        Shape::Square { .. } => {
            // Family 0 is rows and family 1 is columns.
            for (name, family) in [("columns", 1), ("rows", 0)] {
                let lines: Vec<&Vec<Nono>> = puzzle
                    .lane_map()
                    .family(family)
                    .map(|l| &puzzle.lines[l])
                    .collect();
                write_clue_set(&mut res, name, &lines);
            }
        }
        Shape::Triangular(_) => {
            for (name, set) in [
                ("topleft", ClueSet::TopLeft),
                ("bottomleft", ClueSet::BottomLeft),
                ("top", ClueSet::Top),
                ("topright", ClueSet::TopRight),
                ("bottom", ClueSet::Bottom),
                ("bottomright", ClueSet::BottomRight),
            ] {
                let lanes = puzzle.geometry.lanes_in_clue_set(set);
                if lanes.is_empty() {
                    continue; // A sharp corner; webpbn omits the set entirely.
                }
                let lines: Vec<&Vec<Nono>> = lanes.into_iter().map(|l| &puzzle.lines[l]).collect();
                write_clue_set(&mut res, name, &lines);
            }
        }
    }

    res.push_str(r#"</puzzle></puzzleset>"#);
    res.push('\n');

    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::puzzle::PuzzleDynOps;

    /// The worked example from `webpbn_tridder.md`, verbatim.
    pub const DOC_TRIDDLER: &str = r#"<?xml version="1.0"?>
        <puzzleset>
        <puzzle type="triddler" defaultcolor="white">
        <color name="white" char=".">FFFFFF</color>
        <color name="black" char="X">000000</color>
        <clues type="topleft">
        <line><count color="black">1</count><count color="black">1</count><count color="black">1</count></line>
        <line><count color="black">2</count><count color="black">3</count></line>
        </clues>
        <clues type="bottomleft">
        <line><count color="black">1</count></line>
        </clues>
        <clues type="top">
        <line><count color="black">3</count></line>
        <line><count color="black">2</count><count color="black">1</count></line>
        </clues>
        <clues type="topright">
        <line><count color="black">3</count></line>
        </clues>
        <clues type="bottom">
        <line><count color="black">1</count></line>
        <line><count color="black">2</count><count color="black">1</count></line>
        </clues>
        <clues type="bottomright">
        <line><count color="black">3</count></line>
        <line><count color="black">2</count></line>
        </clues>
        </puzzle></puzzleset>"#;

    #[test]
    fn reads_the_doc_triddler() {
        let mut doc = webpbn_to_document(DOC_TRIDDLER).unwrap();
        let puzzle = doc.puzzle().as_tri_nono().unwrap();

        // The outline the six clue-set sizes imply: 16 cells in rows of 5, 6, 5.
        assert_eq!(puzzle.geometry.cell_count(), 16);
        let rows: Vec<usize> = puzzle
            .geometry
            .family(0)
            .map(|i| puzzle.geometry.lane(i).cells.len())
            .collect();
        assert_eq!(rows, vec![5, 6, 5]);

        // Every clue must fit the lane it landed in; `2,3` needs 6 cells, so it pins the
        // assignment.
        for lane in 0..puzzle.geometry.lane_count() {
            let clues = &puzzle.lines[lane];
            let needed: usize = clues.iter().map(|c| c.count as usize).sum::<usize>()
                + clues.len().saturating_sub(1);
            assert!(
                needed <= puzzle.geometry.lane(lane).cells.len(),
                "clues {clues:?} don't fit lane {lane}"
            );
        }
    }

    #[test]
    fn triddler_survives_a_webpbn_round_trip() {
        let mut original = webpbn_to_document(DOC_TRIDDLER).unwrap();
        let serialized = as_webpbn(&original);
        assert!(serialized.contains(r#"type="triddler""#));

        let mut reloaded = webpbn_to_document(&serialized).unwrap();
        assert_eq!(
            original.puzzle().as_tri_nono().unwrap().lines,
            reloaded.puzzle().as_tri_nono().unwrap().lines
        );
        assert_eq!(
            original.puzzle().as_tri_nono().unwrap().geometry,
            reloaded.puzzle().as_tri_nono().unwrap().geometry
        );
    }

    /// This is the test that pinned down the one thing `webpbn_tridder.md` doesn't say: which
    /// end of a line holds clue index 0.
    ///
    /// Of the eight possible combinations of reading direction for the three families, exactly
    /// one makes this example consistent at all, and under that one it solves completely. So
    /// rows and `/` lines read away from their labels, and `\` lines read *towards* theirs.
    #[test]
    fn the_doc_triddler_solves_completely() {
        let mut doc = webpbn_to_document(DOC_TRIDDLER).unwrap();
        let report = doc.puzzle().plain_solve().unwrap();
        assert_eq!(report.cells_left, 0, "should solve by line logic alone");
    }

    /// Guards the direction finding above: flipping any one family must break the puzzle.
    #[test]
    fn no_other_reading_direction_works() {
        for family_to_flip in 0..3 {
            let mut doc = webpbn_to_document(DOC_TRIDDLER).unwrap();
            let mut puzzle = doc.puzzle().as_tri_nono().unwrap().clone();
            for lane in 0..puzzle.geometry.lane_count() {
                if puzzle.geometry.lane(lane).family == family_to_flip {
                    puzzle.lines[lane].reverse();
                }
            }
            let solved_cleanly = matches!(puzzle.plain_solve(), Ok(r) if r.cells_left == 0);
            assert!(
                !solved_cleanly,
                "reversing family {family_to_flip} should not also work"
            );
        }
    }

    #[test]
    fn a_square_puzzle_still_says_grid() {
        let doc = crate::import::char_grid_to_solution("##\n#.");
        let document = crate::puzzle::Document::from_solution(
            crate::puzzle::DynSolution::Square(doc),
            "t.txt".to_string(),
        );
        assert!(as_webpbn(&document).contains(r#"type="grid""#));
    }
}
