use anyhow::bail;
use image::{DynamicImage, GenericImageView, Pixel, Rgba};
use std::{
    char::from_digit,
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    io::Cursor,
    io::Read,
    iter::FromIterator,
    path::PathBuf,
};

use crate::puzzle::{
    self, Clue, ClueStyle, Color, ColorInfo, Corner, Document, DynPuzzle, Nono, NonogramFormat,
    Puzzle, Solution, Triano, BACKGROUND,
};

pub fn load_path(path: &PathBuf, format: Option<NonogramFormat>) -> Document {
    let mut bytes = vec![];
    if path == &PathBuf::from("-") {
        std::io::stdin().read_to_end(&mut bytes).unwrap();
    } else {
        bytes = std::fs::read(path).unwrap();
    }

    load(&path.to_str().unwrap(), bytes, format)
}

pub fn load(filename: &str, bytes: Vec<u8>, format: Option<NonogramFormat>) -> Document {
    let input_format = puzzle::infer_format(&filename, format);

    match input_format {
        NonogramFormat::Html => {
            panic!("HTML input is not supported.")
        }
        NonogramFormat::Image => {
            let img = image::load_from_memory(&bytes).unwrap();
            let solution = image_to_solution(&img);

            Document::new(None, Some(solution), filename.to_string(), None, None)
        }
        NonogramFormat::Webpbn => {
            let webpbn_string = String::from_utf8(bytes).unwrap();
            let mut doc = webpbn_to_document(&webpbn_string);
            doc.file = filename.to_string();
            doc
        }
        NonogramFormat::CharGrid => {
            let grid_string = String::from_utf8(bytes).unwrap();
            let solution = char_grid_to_solution(&grid_string);

            Document::new(None, Some(solution), filename.to_string(), None, None)
        }
        NonogramFormat::Olsak => {
            let olsak_string = String::from_utf8(bytes).unwrap();
            let puzzle = olsak_to_puzzle(&olsak_string).unwrap();

            Document::new(Some(puzzle), None, filename.to_string(), None, None)
        }
    }
}

pub fn image_to_solution(image: &DynamicImage) -> Solution {
    let (width, height) = image.dimensions();

    let mut palette = HashMap::<image::Rgba<u8>, ColorInfo>::new();
    let mut grid: Vec<Vec<Color>> = vec![vec![BACKGROUND; height as usize]; width as usize];

    // pbnsolve output looks weird if the default color isn't called "white".
    palette.insert(
        image::Rgba::<u8>([255, 255, 255, 255]),
        ColorInfo::default_bg(),
    );

    let mut next_char = 'a';
    let mut next_color_idx: u8 = 1; // BACKGROUND is 0

    // Gather the palette
    for y in 0..height {
        for x in 0..width {
            let pixel: Rgba<u8> = image.get_pixel(x, y);
            let color = palette.entry(pixel).or_insert_with(|| {
                let this_char = next_char;
                let [r, g, b] = pixel.channels()[0..3] else {
                    panic!("Image with fewer than three channels?")
                };
                let this_color = Color(next_color_idx);

                next_color_idx += 1;

                if r == 0 && g == 0 && b == 0 {
                    return ColorInfo::default_fg(this_color);
                }

                next_char = (next_char as u8 + 1) as char;

                ColorInfo {
                    ch: this_char,
                    name: format!("{}{}", this_char, format!("{:02X}{:02X}{:02X}", r, g, b)),
                    rgb: (r, g, b),
                    color: this_color,
                    corner: None,
                }
            });

            grid[x as usize][y as usize] = color.color;
        }
    }

    Solution {
        clue_style: ClueStyle::Nono, // Images can't have triangular pixels!
        palette: palette
            .into_values()
            .map(|color_info| (color_info.color, color_info))
            .collect(),
        grid,
    }
}

pub fn char_grid_to_solution(char_grid: &str) -> Solution {
    let mut palette = HashMap::<char, ColorInfo>::new();

    // We want deterministic behavior
    let mut unused_chars = BTreeSet::<char>::new();
    for ch in char_grid.chars() {
        if ch == '\n' {
            continue;
        }
        unused_chars.insert(ch);
    }

    let mut bg_ch: Option<char> = None;

    // Look for a character that seems to represent a white background.
    for possible_bg in [' ', '.', '_', 'w', 'W', '·', '☐', '0', '⬜'] {
        if unused_chars.contains(&possible_bg) {
            bg_ch = Some(possible_bg);
        }
    }

    // But we need to *some* color as background to proceed!
    let bg_ch = match bg_ch {
        Some(x) => x,
        None => {
            eprintln!("number-loom: Warning: unable to guess which character is supposed to be the background; using the upper-left corner");
            char_grid.trim_start().chars().next().unwrap()
        }
    };

    palette.insert(
        bg_ch,
        ColorInfo {
            ch: bg_ch,
            ..ColorInfo::default_bg()
        },
    );
    unused_chars.remove(&bg_ch);

    let mut next_color: u8 = 1;

    // Look for a character that might be black (but it's not required to exist).
    for possible_black in ['#', 'B', 'b', '.', '■', '█', '1', '⬛'] {
        if unused_chars.contains(&possible_black) {
            palette.insert(possible_black, ColorInfo::default_fg(Color(next_color)));
            next_color += 1;
            unused_chars.remove(&possible_black);
            break;
        }
    }

    let lower_right_tri = HashSet::<char>::from_iter(['◢', '🮞', '◿']);
    let lower_left_tri = HashSet::<char>::from_iter(['◣', '🮟', '◺']);
    let upper_left_tri = HashSet::<char>::from_iter(['◤', '🮜', '◸']);
    let upper_right_tri = HashSet::<char>::from_iter(['◥', '🮝', '◹']);
    let mut any_tri = HashSet::<char>::new();
    any_tri.extend(lower_right_tri.iter());
    any_tri.extend(lower_left_tri.iter());
    any_tri.extend(upper_left_tri.iter());
    any_tri.extend(upper_right_tri.iter());

    // By default, use primary and secondary colors:
    let mut unused_colors = BTreeMap::<char, (u8, u8, u8)>::new();
    unused_colors.insert('r', (255, 0, 0));
    unused_colors.insert('g', (0, 255, 0));
    unused_colors.insert('b', (0, 0, 255));

    unused_colors.insert('y', (255, 255, 0));
    unused_colors.insert('c', (0, 255, 255));
    unused_colors.insert('m', (255, 0, 255));

    // Using '🟥' and 'r' in the same puzzle (etc.) will cause a warning.
    unused_colors.insert('🟥', (255, 0, 0));
    unused_colors.insert('🟩', (0, 255, 0));
    unused_colors.insert('🟦', (0, 0, 255));
    unused_colors.insert('🟨', (255, 255, 0));
    unused_colors.insert('🟧', (255, 165, 0));
    unused_colors.insert('🟪', (128, 0, 128));
    unused_colors.insert('🟫', (139, 69, 19));

    for ch in unused_chars {
        if unused_colors.is_empty() {
            // If desperate, use grays and dark colors:
            for i in 1_u8..5_u8 {
                unused_colors.insert(from_digit(i.into(), 10).unwrap(), (44 * i, 44 * i, 44 * i));
            }
            unused_colors.insert('R', (127, 0, 0));
            unused_colors.insert('G', (0, 127, 0));
            unused_colors.insert('B', (0, 0, 127));

            unused_colors.insert('Y', (127, 127, 0));
            unused_colors.insert('C', (0, 127, 127));
            unused_colors.insert('M', (127, 0, 127));
        }
        let rgb = unused_colors
            .remove(&ch)
            .unwrap_or_else(|| unused_colors.pop_first().unwrap().1);

        palette.insert(
            ch,
            ColorInfo {
                ch,
                name: ch.to_string(),
                rgb,
                color: Color(next_color),
                corner: if any_tri.contains(&ch) {
                    Some(Corner {
                        upper: upper_left_tri.contains(&ch) || upper_right_tri.contains(&ch),
                        left: lower_left_tri.contains(&ch) || upper_left_tri.contains(&ch),
                    })
                } else {
                    None
                },
            },
        );
        next_color += 1;
    }

    let mut grid: Vec<Vec<Color>> = vec![];

    // TODO: check that rows are the same length!
    for (y, row) in char_grid
        .split("\n")
        .filter(|line| !line.is_empty())
        .enumerate()
    {
        for (x, ch) in row.chars().enumerate() {
            // There's probably a better way than this...
            grid.resize(std::cmp::max(grid.len(), x + 1), vec![]);
            let new_height = std::cmp::max(grid[x].len(), y + 1);
            grid[x].resize(new_height, BACKGROUND);

            grid[x][y] = palette[&ch].color;
        }
    }

    let has_triangles = palette.values().any(|ci| ci.corner.is_some());

    let clue_style = if has_triangles {
        // Let's assume triano clues are black-and-white; fix the palette!
        for (_, color_info) in &mut palette {
            if color_info.color == BACKGROUND {
                continue;
            }
            color_info.rgb = (0, 0, 0);
        }

        ClueStyle::Triano
    } else {
        ClueStyle::Nono
    };

    Solution {
        clue_style,
        palette: palette
            .into_values()
            .map(|color_info| (color_info.color, color_info))
            .collect(),
        grid,
    }
}

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
    )
}

#[derive(Debug, PartialEq, Eq)]
enum OlsakStanza {
    Preamble,
    Palette,
    Dimension(usize),
}

#[derive(Debug, PartialEq, Eq, Hash)]
enum Glue {
    NoGlue,
    Left,
    Right,
}

pub fn olsak_to_puzzle(olsak: &str) -> anyhow::Result<DynPuzzle> {
    use Glue::*;
    use OlsakStanza::*;
    let mut cur_stanza = Preamble;

    let mut next_color: u8 = 1;

    let named_colors = BTreeMap::<&str, (u8, u8, u8)>::from([
        ("white", (255, 255, 255)),
        ("black", (0, 0, 0)),
        ("red", (255, 0, 0)),
        ("green", (0, 255, 0)),
        ("blue", (0, 0, 255)),
        ("pink", (255, 128, 128)),
        ("yellow", (255, 255, 0)),
        ("r", (255, 0, 0)),
        ("g", (0, 255, 0)),
        ("b", (0, 0, 255)),
    ]);

    let mut olsak_palette = HashMap::<char, ColorInfo>::new();
    // For each dimension, store the "glued" colors (the caps):
    let mut olsak_glued_palettes = vec![
        HashMap::<(char, Glue), ColorInfo>::new(),
        HashMap::<(char, Glue), ColorInfo>::new(),
    ];
    let mut clue_style = ClueStyle::Nono;

    // Dimension > Position > Clue index
    let mut nono_clues: Vec<Vec<Vec<Nono>>> = vec![vec![], vec![]];
    let mut triano_clues: Vec<Vec<Vec<Triano>>> = vec![vec![], vec![]];

    for line in olsak.lines() {
        if let Some(palette_ch) = line.strip_prefix("#") {
            if cur_stanza != Preamble {
                bail!("Palette initiator (line beginning with '#') must be the first content");
            }

            let palette_ch = palette_ch.to_lowercase();

            if palette_ch.starts_with("t") {
                bail!("Triddlers not yet supported!");
            }

            assert!(palette_ch.starts_with("d"));
            cur_stanza = Palette;
        } else if line.starts_with(":") {
            cur_stanza = Dimension(if let Dimension(n) = cur_stanza {
                n + 1
            } else {
                0
            });
        } else if cur_stanza == Preamble {
            /* Just comments */
        } else if cur_stanza == Palette {
            let captures = regex::Regex::new(r"^\s*(\S):(.)\s+(\S+)\s*(.*)$")
                .unwrap()
                .captures(line)
                .ok_or(anyhow::anyhow!("Malformed palette line {line}"))?;

            let (_, [input_ch, unique_ch, color_name, comment]) = captures.extract();

            let parse_glue = |c| match c {
                '>' => Right,
                '<' => Left,
                _ => NoGlue,
            };

            let rising = color_name.contains('/');

            let (corner, unique_ch) = match (color_name.split_once(&['/', '\\']), rising) {
                (None, _) => (None, unique_ch.chars().next().unwrap()),
                (Some(("white", "black")), true) => (
                    Some(Corner {
                        upper: false,
                        left: false,
                    }),
                    '◢',
                ),
                (Some(("white", "black")), false) => (
                    Some(Corner {
                        upper: true,
                        left: false,
                    }),
                    '◥',
                ),
                (Some(("black", "white")), true) => (
                    Some(Corner {
                        upper: true,
                        left: true,
                    }),
                    '◤',
                ),
                (Some(("black", "white")), false) => (
                    Some(Corner {
                        upper: false,
                        left: true,
                    }),
                    '◣',
                ),
                (Some((_, _)), _) => {
                    eprintln!("Unsupported triangle color combination: {color_name}");
                    (None, unique_ch.chars().next().unwrap())
                }
            };

            let rgb = if let Some((_, [rs, gs, bs])) = regex::Regex::new(r"^#(..)(..)(..)$")
                .unwrap()
                .captures(color_name)
                .map(|c| c.extract())
            {
                (
                    u8::from_str_radix(rs, 16).unwrap(),
                    u8::from_str_radix(gs, 16).unwrap(),
                    u8::from_str_radix(bs, 16).unwrap(),
                )
            } else if corner.is_some() {
                (0, 0, 0) // Assumes Triano puzzles are black-and-white!
            } else if let Some((r, g, b)) = named_colors.get(color_name) {
                (*r, *g, *b)
            } else if let Some((r, g, b)) = named_colors.get(input_ch) {
                (*r, *g, *b)
            } else {
                // TODO: generate nice colors, like for chargrid (probably less critical here)
                (128, 128, 128)
            };

            let dim_0_glue = comment.chars().nth(0).map(parse_glue).unwrap_or(NoGlue);
            let dim_1_glue = comment.chars().nth(1).map(parse_glue).unwrap_or(NoGlue);

            if dim_0_glue != NoGlue || dim_1_glue != NoGlue {
                clue_style = ClueStyle::Triano;
            }

            let color = if input_ch == "0" {
                BACKGROUND
            } else {
                Color(next_color)
            };

            let color_info = ColorInfo {
                ch: unique_ch,
                name: color_name.to_string(),
                rgb,
                color,
                corner,
            };
            let input_ch = input_ch.chars().next().unwrap();

            if dim_0_glue == NoGlue && dim_1_glue == NoGlue {
                olsak_palette.insert(input_ch, color_info);
            } else {
                assert!(dim_0_glue != NoGlue && dim_1_glue != NoGlue);
                olsak_glued_palettes[0].insert((input_ch, dim_0_glue), color_info.clone());
                olsak_glued_palettes[1].insert((input_ch, dim_1_glue), color_info);
            }

            next_color += 1;
        } else if let Dimension(d) = cur_stanza {
            if !olsak_palette.contains_key(&'1') {
                olsak_palette.insert(
                    '1',
                    ColorInfo {
                        ch: '#',
                        name: "black".to_string(),
                        rgb: (0, 0, 0),
                        color: Color(next_color),
                        corner: None,
                    },
                );
            }

            if d >= 2 {
                // There can be comments after the end!
                continue;
            }
            let clue_strs = line.split_whitespace();
            match clue_style {
                ClueStyle::Nono => {
                    let mut clues = vec![];
                    for clue_str in clue_strs {
                        if let Ok(count) = clue_str.parse::<u16>() {
                            clues.push(Nono {
                                color: olsak_palette[&'1'].color,
                                count,
                            })
                        } else {
                            let count: u8 = clue_str
                                .trim_end_matches(|c: char| !c.is_numeric())
                                .parse()?;
                            let input_ch = clue_str.chars().last().unwrap();
                            clues.push(Nono {
                                color: olsak_palette[&input_ch].color,
                                count: count as u16,
                            })
                        }
                    }
                    nono_clues[d].push(clues);
                }
                ClueStyle::Triano => {
                    let mut clues = vec![];

                    for clue_str in clue_strs {
                        let mut chars: Vec<char> = clue_str.chars().collect();
                        let front_cap = chars
                            .first()
                            .map(|c| olsak_glued_palettes[d].get(&(*c, Left)).map(|c| c.color))
                            .flatten();
                        if front_cap.is_some() {
                            chars.remove(0);
                        }
                        let back_cap = chars
                            .last()
                            .map(|c| olsak_glued_palettes[d].get(&(*c, Right)).map(|c| c.color))
                            .flatten();
                        if back_cap.is_some() {
                            chars.pop();
                        }
                        let body_color = if !chars.last().unwrap().is_numeric() {
                            olsak_palette[&chars.pop().unwrap()].color
                        } else {
                            olsak_palette[&'1'].color
                        };

                        let body_len = chars.iter().collect::<String>().parse::<u16>()?
                            - (front_cap.is_some() as u16 + back_cap.is_some() as u16);

                        clues.push(Triano {
                            front_cap,
                            body_len,
                            body_color,
                            back_cap,
                        });
                    }
                    triano_clues[d].push(clues);
                }
            }
        }
    }
    if !olsak_palette.contains_key(&'0') {
        olsak_palette.insert('0', ColorInfo::default_bg());
    }

    let mut palette: HashMap<Color, ColorInfo> = olsak_palette
        .into_values()
        .map(|ci| (ci.color, ci))
        .collect();
    for d in 0..2 {
        for (_, ci) in olsak_glued_palettes[d].iter() {
            palette.insert(ci.color, ci.clone());
        }
    }

    Ok(match clue_style {
        ClueStyle::Nono => DynPuzzle::Nono(Puzzle::<Nono> {
            palette,
            rows: nono_clues[0].clone(),
            cols: nono_clues[1].clone(),
        }),
        ClueStyle::Triano => DynPuzzle::Triano(Puzzle::<Triano> {
            palette,
            rows: triano_clues[0].clone(),
            cols: triano_clues[1].clone(),
        }),
    })
}

pub fn quality_check(solution: &Solution) {
    let width = solution.grid.len();
    let height = solution.grid.first().unwrap().len();

    let bg_squares_found: usize = solution
        .grid
        .iter()
        .map(|col| {
            col.iter()
                .map(|c| if *c == BACKGROUND { 1 } else { 0 })
                .sum::<usize>()
        })
        .sum();

    if bg_squares_found < (width + height) {
        eprintln!(
            "number-loom: warning: {} is a very small number of background squares",
            bg_squares_found
        );
    }

    if (width * height - bg_squares_found) < (width + height) {
        eprintln!(
            "number-loom: warning: {} is a very small number of foreground squares",
            width * height - bg_squares_found
        );
    }

    let num_colors = solution.palette.len();
    if num_colors > 30 {
        panic!(
            "{} colors detected. Nonograms with more than 30 colors are not supported.",
            num_colors
        );
    } else if num_colors > 10 {
        eprintln!(
            "number-loom: {} colors detected. That's probably too many.",
            num_colors
        )
    }

    // Find similar colors
    for (color_key, color) in &solution.palette {
        for (color_key2, color2) in &solution.palette {
            if color_key == color_key2 {
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
                eprintln!(
                    "number-loom: warning: very similar colors found: {:?} and {:?}",
                    color.rgb, color2.rgb
                );
            }
        }
    }
}

pub fn solution_to_triano_puzzle(solution: &Solution) -> Puzzle<Triano> {
    let width = solution.grid.len();
    let height = solution.grid.first().unwrap().len();

    let mut rows: Vec<Vec<Triano>> = Vec::new();
    let mut cols: Vec<Vec<Triano>> = Vec::new();

    let blank_clue = Triano {
        front_cap: None,
        body_color: BACKGROUND,
        body_len: 0,
        back_cap: None,
    };

    // Generate row clues
    for y in 0..height {
        let mut clues = Vec::<Triano>::new();
        let mut cur_clue = blank_clue;

        for x in 0..width {
            let color = solution.grid[x][y];
            let color_info = &solution.palette[&color];

            // For example `!left` means ◢ or ◥:
            if color_info.corner.is_some_and(|c| !c.left) {
                // Only a blank clue can accept a front cap:
                if cur_clue != blank_clue {
                    clues.push(cur_clue);
                    cur_clue = blank_clue
                }
                cur_clue.front_cap = Some(color);
            } else if color_info.corner.is_some_and(|c| c.left) {
                // The back cap is always none...
                cur_clue.back_cap = Some(color);
                // ...because we finish right after setting it
                clues.push(cur_clue);
                cur_clue = blank_clue;
            } else if color == BACKGROUND {
                if cur_clue != blank_clue {
                    clues.push(cur_clue);
                    cur_clue = blank_clue;
                }
            } else {
                // Since the back cap is always none, the only obstacle to continuing is if the
                // body color is wrong.
                if cur_clue.body_color != BACKGROUND && cur_clue.body_color != color {
                    clues.push(cur_clue);
                    cur_clue = blank_clue;
                }
                cur_clue.body_color = color;
                cur_clue.body_len += 1;
            }
        }
        if cur_clue != blank_clue {
            clues.push(cur_clue);
        }

        rows.push(clues);
    }

    // Generate column clues
    for x in 0..width {
        let mut clues = Vec::<Triano>::new();
        let mut cur_clue = blank_clue;

        for y in 0..height {
            let color = solution.grid[x][y];
            let color_info = &solution.palette[&color];

            if color_info.corner.is_some_and(|c| !c.upper) {
                // Only a blank clue can accept a front cap:
                if cur_clue != blank_clue {
                    clues.push(cur_clue);
                    cur_clue = blank_clue
                }
                cur_clue.front_cap = Some(color);
            } else if color_info.corner.is_some_and(|c| c.upper) {
                // The back cap is always none...
                cur_clue.back_cap = Some(color);
                // ...because we finish right after setting it
                clues.push(cur_clue);
                cur_clue = blank_clue;
            } else if color == BACKGROUND {
                if cur_clue != blank_clue {
                    clues.push(cur_clue);
                    cur_clue = blank_clue;
                }
            } else {
                // Since the back cap is always none, the only obstacle to continuing is if the
                // body color is wrong.
                if cur_clue.body_color != BACKGROUND && cur_clue.body_color != color {
                    clues.push(cur_clue);
                    cur_clue = blank_clue;
                }
                cur_clue.body_color = color;
                cur_clue.body_len += 1;
            }
        }
        if cur_clue != blank_clue {
            clues.push(cur_clue);
        }

        cols.push(clues);
    }

    Puzzle {
        palette: solution.palette.clone(),
        rows,
        cols,
    }
}

pub fn solution_to_puzzle(solution: &Solution) -> Puzzle<Nono> {
    let width = solution.grid.len();
    let height = solution.grid.first().unwrap().len();

    let mut rows: Vec<Vec<Nono>> = Vec::new();
    let mut cols: Vec<Vec<Nono>> = Vec::new();

    // Generate row clues
    for y in 0..height {
        let mut clues = Vec::<Nono>::new();

        let mut prev_color: Option<Color> = None;
        let mut run = 1;
        for x in 0..width + 1 {
            let color = if x < width {
                Some(solution.grid[x][y])
            } else {
                None
            };
            if prev_color == color {
                run += 1;
                continue;
            }
            match prev_color {
                None => {}
                Some(color) if color == puzzle::BACKGROUND => {}
                Some(color) => clues.push(Nono { color, count: run }),
            }
            prev_color = color;
            run = 1;
        }
        rows.push(clues);
    }

    // Generate column clues
    for x in 0..width {
        let mut clues = Vec::<Nono>::new();

        let mut prev_color = None;
        let mut run = 1;
        for y in 0..height + 1 {
            let color = if y < height {
                Some(solution.grid[x][y])
            } else {
                None
            };
            if prev_color == color {
                run += 1;
                continue;
            }
            match prev_color {
                None => {}
                Some(color) if color == BACKGROUND => {}
                Some(color) => clues.push(Nono { color, count: run }),
            }
            prev_color = color;
            run = 1;
        }
        cols.push(clues);
    }

    Puzzle {
        palette: solution.palette.clone(),
        rows,
        cols,
    }
}

pub fn bw_palette() -> HashMap<Color, ColorInfo> {
    let mut palette = HashMap::new();
    palette.insert(BACKGROUND, ColorInfo::default_bg());
    palette.insert(Color(1), ColorInfo::default_fg(Color(1)));
    palette
}

pub fn load_zip_from_url(url: &str) -> anyhow::Result<Vec<Document>> {
    let response = reqwest::blocking::get(url)?;
    let zip_bytes = response.bytes()?;
    let zip_cursor = Cursor::new(zip_bytes);

    let mut archive = zip::ZipArchive::new(zip_cursor)?;
    let mut documents = vec![];

    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        let filename = file.name().to_string();

        if file.is_dir() {
            continue;
        }

        let mut bytes = vec![];
        file.read_to_end(&mut bytes)?;
        documents.push(load(&filename, bytes, None));
    }

    Ok(documents)
}

pub fn triano_palette() -> HashMap<Color, ColorInfo> {
    let mut palette = HashMap::new();
    palette.insert(BACKGROUND, ColorInfo::default_bg());
    palette.insert(Color(1), ColorInfo::default_fg(Color(1)));

    palette.insert(
        Color(3),
        ColorInfo {
            ch: '◤',
            name: r#"black/white"#.to_string(),
            rgb: (0, 0, 0),
            color: Color(3),
            corner: Some(Corner {
                upper: true,
                left: true,
            }),
        },
    );
    palette.insert(
        Color(4),
        ColorInfo {
            ch: '◥',
            name: r#"white\black"#.to_string(),
            rgb: (0, 0, 0),
            color: Color(4),
            corner: Some(Corner {
                upper: true,
                left: false,
            }),
        },
    );
    palette.insert(
        Color(5),
        ColorInfo {
            ch: '◣',
            name: r#"black\white"#.to_string(),
            rgb: (0, 0, 0),
            color: Color(5),
            corner: Some(Corner {
                upper: false,
                left: true,
            }),
        },
    );
    palette.insert(
        Color(6),
        ColorInfo {
            ch: '◢',
            name: r#"white/black"#.to_string(),
            rgb: (0, 0, 0),
            color: Color(6),
            corner: Some(Corner {
                upper: false,
                left: false,
            }),
        },
    );

    palette
}
