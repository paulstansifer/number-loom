use std::collections::{BTreeMap, HashMap};

use anyhow::bail;

use crate::puzzle::{
    ClueStyle, Color, ColorInfo, Corner, DynPuzzle, Nono, Puzzle, Triano, BACKGROUND,
};

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

pub fn as_olsak_nono(puzzle: &Puzzle<Nono>) -> String {
    let mut orig_to_sanitized: HashMap<char, char> = HashMap::new();

    let mut palette = puzzle.palette.clone();

    let mut res = String::new();
    res.push_str("#d\n");

    // Nonny doesn't like it if white isn't the first color in the palette.
    res.push_str("   0:   #FFFFFF   white\n");
    for color in palette.values_mut() {
        if color.rgb != (255, 255, 255) {
            let (r, g, b) = color.rgb;
            color.ch = olsak_ch(color.ch, &mut orig_to_sanitized);
            let ch = color.ch;
            let (spec, comment) = (&format!("#{r:02X}{g:02X}{b:02X}"), color.name.to_string());

            // I think the second `ch` can perhaps be any ASCII character.
            res.push_str(&format!("   {ch}:{ch}  {spec}   {comment}\n",));
        }
    }
    res.push_str(": rows\n");
    for row in &puzzle.rows {
        for clue in row {
            res.push_str(&format!("{}{} ", clue.count, palette[&clue.color].ch));
        }
        res.push('\n');
    }
    res.push_str(": columns\n");
    for column in &puzzle.cols {
        for clue in column {
            res.push_str(&format!("{}{} ", clue.count, palette[&clue.color].ch));
        }
        res.push('\n');
    }

    res
}

pub fn as_olsak_triano(puzzle: &Puzzle<Triano>) -> String {
    use crate::puzzle::{self, Corner};


    let mut orig_to_sanitized: HashMap<char, char> = HashMap::new();

    let mut res = String::new();
    res.push_str("#d\n");

    let palette = puzzle
        .palette
        .iter()
        .map(|(color, color_info)| {
            (
                color,
                puzzle::ColorInfo {
                    ch: olsak_ch(color_info.ch, &mut orig_to_sanitized),
                    ..color_info.clone()
                },
            )
        })
        .collect::<HashMap<_, _>>();

    // Nonny doesn't like it if white isn't the first color in the palette.
    res.push_str("   0:   #FFFFFF   white\n");
    for color in palette.values() {
        if color.rgb != (255, 255, 255) {
            let (r, g, b) = color.rgb;
            let ch = color.ch;
            let (spec, comment) = match color.corner {
                None => (&format!("#{r:02X}{g:02X}{b:02X}"), color.name.to_string()),
                Some(Corner { upper, left }) => (
                    &format!(
                        "{}{}{}",
                        if left { "black" } else { "white" },
                        if left == upper { "/" } else { "\\" },
                        if left { "white" } else { "black" },
                    ),
                    format!(
                        "{}{}",
                        if left { ">" } else { "<" },
                        if upper { ">" } else { "<" }
                    ),
                ),
            };

            // I think the second `ch` can perhaps be any ASCII character.
            res.push_str(&format!("   {ch}:{ch}  {spec}   {comment}\n",));
        }
    }
    res.push_str(": rows\n");
    for row in &puzzle.rows {
        for clue in row {
            if let Some(c) = clue.front_cap {
                res.push(palette[&c].ch);
            }
            res.push_str(&format!(
                "{}{}",
                clue.body_len + (clue.front_cap.is_some() as u16 + clue.back_cap.is_some() as u16),
                palette[&clue.body_color].ch
            ));
            if let Some(c) = clue.back_cap {
                res.push(palette[&c].ch);
            }
            res.push(' ');
        }
        res.push('\n');
    }
    res.push_str(": columns\n");
    for column in &puzzle.cols {
        for clue in column {
            if let Some(c) = clue.front_cap {
                res.push(palette[&c].ch);
            }
            res.push_str(&format!(
                "{}{}",
                clue.body_len + (clue.front_cap.is_some() as u16 + clue.back_cap.is_some() as u16),
                palette[&clue.body_color].ch
            ));
            if let Some(c) = clue.back_cap {
                res.push(palette[&c].ch);
            }
            res.push(' ');
        }
        res.push('\n');
    }

    res
}

pub fn olsak_ch(c: char, orig_to_sanitized: &mut HashMap<char, char>) -> char {
    use std::{collections::HashSet, iter::FromIterator};
    let existing = HashSet::<char>::from_iter(orig_to_sanitized.values().cloned());
    *orig_to_sanitized.entry(c).or_insert_with(|| {
        if c.is_alphanumeric() && !existing.contains(&c) {
            return c;
        } else {
            for c in 'a'..'z' {
                if !existing.contains(&c) {
                    return c;
                }
            }
            panic!("too many colors!")
        }
    })
}
