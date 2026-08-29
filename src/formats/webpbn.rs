use anyhow::{Context, bail};
use std::collections::HashMap;

use crate::geometry::{ClueSet, ClueSetCounts, GridKind, Outline, Shape, Tri};
use crate::puzzle::{
    BACKGROUND, ClueStyle, Color, ColorInfo, Document, DynPuzzle, DynSolution, Nono, Puzzle,
    Solution,
};

fn get_children<'a, 'input>(
    node: roxmltree::Node<'a, 'input>,
    tag: &str,
) -> anyhow::Result<Vec<roxmltree::Node<'a, 'input>>> {
    let mut res = vec![];

    for child in node.children() {
        if child.is_text() && child.text().unwrap().trim() != "" {
            bail!("unexpected text: {}", child.text().unwrap());
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

/// Like `get_single_child`, but tolerant of siblings with other tags, and of there being more than
/// one match (it takes the first).
///
/// A `<puzzleset>` may carry its own metadata (`<source>`, `<title>`, ...) alongside the puzzles,
/// and may hold several puzzles — which is why `get_children`'s strictness is wrong at that level,
/// even though it's just right inside a `<puzzle>`.
fn find_first_child<'a, 'input>(
    node: roxmltree::Node<'a, 'input>,
    tag: &str,
) -> anyhow::Result<roxmltree::Node<'a, 'input>> {
    node.children()
        .find(|child| child.is_element() && child.tag_name().name() == tag)
        .with_context(|| format!("did not find the element {tag}"))
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

/// Parses a `<solution><image>` body into cell colors, in dense (row-major) order.
///
/// Grid puzzles delimit each row with `|`; triddlers delimit each row with `/` or `\`, chosen
/// per-row to match the slope of that row's ends (see `webpbn_tridder.md`). We don't need to
/// know which delimiter means what, though: treating all three characters as row boundaries and
/// keeping only the non-blank segments between them recovers the rows in top-to-bottom order
/// either way, and rows read left-to-right — the same order `Geometry`'s dense numbering uses.
fn parse_solution_image(
    text: &str,
    ch_to_color: &HashMap<char, Color>,
) -> anyhow::Result<Vec<Color>> {
    let mut cells = vec![];
    for row in text.split(['|', '/', '\\']) {
        if row.trim().is_empty() {
            continue;
        }
        for ch in row.chars() {
            let color = ch_to_color
                .get(&ch)
                .with_context(|| format!("solution image uses undefined color char: {ch}"))?;
            cells.push(*color);
        }
    }
    Ok(cells)
}

fn solution_from_image<K: GridKind>(
    palette: HashMap<Color, ColorInfo>,
    geometry: crate::geometry::Geometry<K>,
    cells: Vec<Color>,
) -> anyhow::Result<Solution<K>> {
    anyhow::ensure!(
        cells.len() == geometry.cell_count(),
        "solution image has {} cells but the puzzle has {}",
        cells.len(),
        geometry.cell_count()
    );
    Ok(Solution::new(ClueStyle::Nono, palette, geometry, cells))
}

pub fn webpbn_to_document(webpbn: &str) -> anyhow::Result<Document> {
    // Wolter's sample puzzles all declare `<!DOCTYPE pbn SYSTEM "http://webpbn.com/pbn-0.3.dtd">`,
    // which roxmltree rejects unless asked otherwise. It still won't fetch external entities, so
    // this only means "tolerate the declaration", not "go to the network".
    let doc = roxmltree::Document::parse_with_options(
        webpbn,
        roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        },
    )
    .context("could not parse XML")?;
    let puzzleset = doc.root_element();
    let puzzle_node = find_first_child(puzzleset, "puzzle")?;

    let mut title = None;
    let mut description = None;
    let mut author = None;
    let mut authorid = None;
    let mut id = None;
    let mut license = None;

    // webpbn keeps two separate notions here, and conflating them mis-reads most real files.
    // `backgroundcolor` names the blank cell; `defaultcolor` names what a `<count>` means when it
    // doesn't say. Both are optional, with the defaults below. Wolter's sample set, for instance,
    // is almost entirely `defaultcolor="black"` with the background left implicit.
    let background_color = puzzle_node.attribute("backgroundcolor").unwrap_or("white");
    let default_clue_color = puzzle_node.attribute("defaultcolor").unwrap_or("black");
    let mut next_color_index = 1;

    let mut named_colors = HashMap::<String, Color>::new();
    let mut ch_to_color = HashMap::<char, Color>::new();

    let mut palette = HashMap::<Color, ColorInfo>::new();
    let mut rows: Vec<Vec<Nono>> = vec![];
    let mut cols: Vec<Vec<Nono>> = vec![];
    // Triddlers split each of their three clue directions across two `<clues>` sets.
    let mut triddler_clues: HashMap<ClueSet, Vec<Vec<Nono>>> = HashMap::new();
    // The `<solution type="goal">` image, if the file bothered to include one — a puzzle that
    // isn't line-solvable has no other way for us to learn its answer.
    let mut goal_solution: Option<Vec<Color>> = None;

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
            let color = if color_name == background_color {
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

            let ch = puzzle_part
                .attribute("char")
                .context("color element missing 'char' attribute")?
                .chars()
                .next()
                .context("'char' attribute is empty")?;

            let color_info = ColorInfo {
                // TODO: error if there's more than one char!
                ch,
                name: color_name.to_string(),
                rgb: (r, g, b),
                color,
                corner: None, // webpbn isn't intended to represent Triano clues
            };

            palette.insert(color, color_info);
            named_colors.insert(color_name.to_string(), color);
            ch_to_color.insert(ch, color);
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
                    let color_name = block.attribute("color").unwrap_or(default_clue_color);
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
        } else if tag_name == "solution" {
            // webpbn also allows `type="saved"`/`"solution"` for user snapshots; only the
            // designer's intended answer is any use to us.
            let solution_type = puzzle_part.attribute("type").unwrap_or("goal");
            if solution_type == "goal" {
                let image = find_first_child(puzzle_part, "image")?;
                let text: String = image
                    .children()
                    .filter(|n| n.is_text())
                    .filter_map(|n| n.text())
                    .collect::<Vec<_>>()
                    .join("");
                goal_solution = Some(parse_solution_image(&text, &ch_to_color)?);
            }
        }
    }

    let (puzzle, solution): (DynPuzzle, Option<DynSolution>) = if triddler {
        let p = triddler_puzzle(palette.clone(), &triddler_clues)?;
        let solution = goal_solution
            .map(|cells| solution_from_image(palette, p.geometry.clone(), cells))
            .transpose()?
            .map(DynSolution::Tri);
        (p.into(), solution)
    } else {
        let p = Puzzle::square(palette.clone(), rows, cols);
        let solution = goal_solution
            .map(|cells| solution_from_image(palette, p.geometry.clone(), cells))
            .transpose()?
            .map(DynSolution::Square);
        (p.into(), solution)
    };

    Ok(Document::new(
        Some(puzzle),
        solution,
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

    // Name the background explicitly rather than assuming it's called "white": a palette lifted
    // from a PNG names its colors after their hex values, and a reader that guesses "white" would
    // treat every one of them as foreground.
    let background_name = &palette[&BACKGROUND].name;
    // Every `<count>` we write names its own color, so `defaultcolor` is never actually consulted;
    // it just has to name a real color for readers that validate it.
    // Lowest color index rather than whatever the `HashMap` yields first, so the output is stable.
    let default_clue_name = palette
        .values()
        .filter(|c| c.color != BACKGROUND)
        .min_by_key(|c| c.color)
        .map_or(background_name, |c| &c.name);

    let mut res = String::new();
    // If you add <!DOCTYPE pbn SYSTEM "https://webpbn.com/pbn-0.3.dtd">, `pbnsolve` emits a warning.
    res.push_str(&format!(
        indoc! {r#"
        <?xml version="1.0"?>
        <puzzleset>
        <puzzle type="{}" backgroundcolor="{}" defaultcolor="{}">
        <source>number-loom</source>
        "#},
        puzzle_type, background_name, default_clue_name,
    ));
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

    /// Everything about a file from webpbn.com that the reader used to choke on, in one puzzle:
    /// a `<!DOCTYPE>`, a `<source>` sitting beside `<puzzle>` inside the `<puzzleset>`,
    /// `defaultcolor` naming the *foreground*, no `backgroundcolor` at all, and `<count>` elements
    /// that leave their color implicit.
    const WEBPBN_HOUSE_STYLE: &str = r#"<?xml version="1.0"?>
        <!DOCTYPE pbn SYSTEM "http://webpbn.com/pbn-0.3.dtd">
        <puzzleset>
        <source>webpbn.com</source>
        <puzzle type="grid" defaultcolor="black">
        <title>Two by two</title>
        <note>published</note>
        <color name="white" char=".">FFFFFF</color>
        <color name="black" char="X">000000</color>
        <clues type="columns">
        <line><count>2</count></line>
        <line><count>1</count></line>
        </clues>
        <clues type="rows">
        <line><count>2</count></line>
        <line><count>1</count></line>
        </clues>
        </puzzle></puzzleset>"#;

    #[test]
    fn reads_a_file_in_webpbn_house_style() {
        let mut doc = webpbn_to_document(WEBPBN_HOUSE_STYLE).unwrap();
        assert_eq!(doc.title, "Two by two");

        let puzzle = doc.puzzle().as_square_nono().unwrap();
        // `defaultcolor="black"` must land on the clues, not on the background: the unqualified
        // `<count>`s are black, and white — never mentioned as a default — is the blank cell.
        assert_ne!(palette_color(&puzzle.palette, "black"), BACKGROUND);
        assert_eq!(palette_color(&puzzle.palette, "white"), BACKGROUND);
        for line in &puzzle.lines {
            for clue in line {
                assert_ne!(clue.color, BACKGROUND, "clues shouldn't be background");
            }
        }
    }

    #[test]
    fn a_file_in_webpbn_house_style_solves() {
        let mut doc = webpbn_to_document(WEBPBN_HOUSE_STYLE).unwrap();
        assert_eq!(doc.puzzle().plain_solve().unwrap().cells_left, 0);
    }

    /// A 2x2 grid where every clue is a lone `1` — line logic alone can't place any of them
    /// (each line just knows "one cell somewhere in two"), but the diagonal solution is unique
    /// among the two the clues alone allow, so a file that bothers to include `<solution>` should
    /// still load as fully solved.
    const AMBIGUOUS_WITH_SOLUTION: &str = r#"<?xml version="1.0"?>
        <puzzleset>
        <puzzle type="grid" backgroundcolor="white" defaultcolor="black">
        <color name="white" char=".">FFFFFF</color>
        <color name="black" char="X">000000</color>
        <clues type="columns">
        <line><count>1</count></line>
        <line><count>1</count></line>
        </clues>
        <clues type="rows">
        <line><count>1</count></line>
        <line><count>1</count></line>
        </clues>
        <solution type="goal">
        <image>
        |X.|
        |.X|
        </image>
        </solution>
        </puzzle></puzzleset>"#;

    #[test]
    fn line_logic_alone_cannot_solve_the_ambiguous_fixture() {
        let mut doc = webpbn_to_document(AMBIGUOUS_WITH_SOLUTION).unwrap();
        assert!(doc.puzzle().plain_solve().unwrap().cells_left > 0);
    }

    #[test]
    fn reads_the_embedded_solution_for_a_puzzle_line_logic_cant_finish() {
        let mut doc = webpbn_to_document(AMBIGUOUS_WITH_SOLUTION).unwrap();
        assert!(doc.has_complete_solution().unwrap());

        let solution = doc.solution().unwrap().as_square().unwrap();
        let black = palette_color(&solution.palette, "black");
        assert_eq!(solution.get((0, 0)), Some(black));
        assert_eq!(solution.get((1, 0)), Some(BACKGROUND));
        assert_eq!(solution.get((0, 1)), Some(BACKGROUND));
        assert_eq!(solution.get((1, 1)), Some(black));
    }

    fn palette_color(palette: &HashMap<Color, ColorInfo>, name: &str) -> Color {
        palette
            .values()
            .find(|c| c.name == name)
            .unwrap_or_else(|| panic!("no color named {name}"))
            .color
    }

    /// A palette lifted from a PNG names its colors after their hex values, so nothing in it is
    /// called "white". The background has to survive a round trip anyway.
    #[test]
    fn an_oddly_named_background_survives_a_round_trip() {
        let solution = crate::import::char_grid_to_solution("##\n#.");
        let mut document = crate::puzzle::Document::from_solution(
            crate::puzzle::DynSolution::Square(solution),
            "t.txt".to_string(),
        );
        let original_bg_name = document.puzzle().palette()[&BACKGROUND].name.clone();

        let serialized = as_webpbn(&document);
        assert!(
            serialized.contains(&format!(r#"backgroundcolor="{original_bg_name}""#)),
            "should name its background explicitly: {serialized}"
        );

        let mut reloaded = webpbn_to_document(&serialized).unwrap();
        assert_eq!(
            palette_color(reloaded.puzzle().palette(), &original_bg_name),
            BACKGROUND
        );
        assert_eq!(
            document.puzzle().as_square_nono().unwrap().lines,
            reloaded.puzzle().as_square_nono().unwrap().lines
        );
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
