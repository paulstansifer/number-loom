use std::collections::HashMap;

use anyhow::bail;

use crate::puzzle::{
    Clue, Color, ColorInfo, Document, Nono, Puzzle, BACKGROUND,
};

pub fn get_children<'a, 'input>(
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

pub fn get_single_child<'a, 'input>(
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

pub fn webpbn_to_document(webpbn: &str) -> Document {
    let doc = roxmltree::Document::parse(webpbn).unwrap();
    let puzzleset = doc.root_element();
    let puzzle_node = get_single_child(puzzleset, "puzzle").unwrap();

    let mut title = None;
    let mut description = None;
    let mut author = None;
    let mut authorid = None;
    let mut id = None;
    let mut license = None;

    let default_color = puzzle_node
        .attribute("defaultcolor")
        .expect("Expected a 'defaultcolor");
    let mut next_color_index = 1;

    let mut named_colors = HashMap::<String, Color>::new();

    let mut puzzle = Puzzle {
        palette: HashMap::<Color, ColorInfo>::new(),
        rows: vec![],
        cols: vec![],
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
            let color_name = puzzle_part.attribute("name").unwrap();
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

            let color_text = puzzle_part.text().expect("Expected hex color in text");
            let (_, component_strs) = hex_color
                .captures(&color_text)
                .expect("Expected a string of 6 hex digits")
                .extract();

            let [r, g, b] = component_strs.map(|s| u8::from_str_radix(s, 16).unwrap());

            let color_info = ColorInfo {
                // TODO: error if there's more than one char!
                ch: puzzle_part
                    .attribute("char")
                    .unwrap()
                    .chars()
                    .next()
                    .unwrap(),
                name: color_name.to_string(),
                rgb: (r, g, b),
                color: color,
                corner: None, // webpbn isn't intended to represent Triano clues
            };

            puzzle.palette.insert(color, color_info);
            named_colors.insert(color_name.to_string(), color);
        } else if tag_name == "clues" {
            let row = if puzzle_part.attribute("type") == Some("rows") {
                true
            } else if puzzle_part.attribute("type") == Some("columns") {
                false
            } else {
                panic!("Expected rows or columns.")
            };

            let mut clue_lanes = vec![];

            for lane in get_children(puzzle_part, "line").unwrap() {
                let mut clues = vec![];
                for block in get_children(lane, "count").unwrap() {
                    clues.push(Nono {
                        color: named_colors[block
                            .attribute("color")
                            .expect("Expected 'color' attribute")],
                        count: u16::from_str_radix(&block.text().unwrap(), 10)
                            .expect("Expected a number."),
                    });
                }
                clue_lanes.push(clues);
            }

            if row {
                puzzle.rows = clue_lanes;
            } else {
                puzzle.cols = clue_lanes;
            }
        }
    }

    Document::new(
        Some(Nono::to_dyn(puzzle)),
        None,
        "".to_string(),
        title,
        description,
        author.or(authorid),
        id,
        license,
    )
}

pub fn as_webpbn(document: &Document) -> String {
    use indoc::indoc;

    let mut document_with_puzzle = document.clone();
    let puzzle = document_with_puzzle.puzzle().assume_nono();

    let mut res = String::new();
    // If you add <!DOCTYPE pbn SYSTEM "https://webpbn.com/pbn-0.3.dtd">, `pbnsolve` emits a warning.
    res.push_str(indoc! {r#"
        <?xml version="1.0"?>
        <puzzleset>
        <puzzle type="grid" defaultcolor="white">
        <source>number-loom</source>
        "#});
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
    for color in puzzle.palette.values() {
        let (r, g, b) = color.rgb;
        res.push_str(&format!(
            r#"<color name="{}" char="{}">{:02X}{:02X}{:02X}</color>"#,
            color.name, color.ch, r, g, b
        ));
        res.push('\n');
    }

    res.push_str(r#"<clues type="columns">"#);
    for column in &puzzle.cols {
        res.push_str("<line>");
        for clue in column {
            res.push_str(&format!(
                r#"<count color="{}">{}</count>"#,
                puzzle.palette[&clue.color].name, clue.count
            ));
        }
        res.push_str("</line>\n");
    }
    res.push_str(r#"</clues>"#);
    res.push('\n');

    res.push_str(r#"<clues type="rows">"#);
    for row in &puzzle.rows {
        res.push_str("<line>");
        for clue in row {
            res.push_str(&format!(
                r#"<count color="{}">{}</count>"#,
                puzzle.palette[&clue.color].name, clue.count
            ));
        }
        res.push_str("</line>\n");
    }
    res.push_str(r#"</clues>"#);
    res.push('\n');

    res.push_str(r#"</puzzle></puzzleset>"#);
    res.push('\n');

    res
}
