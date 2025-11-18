use std::{
    collections::{HashMap, HashSet},
    iter::FromIterator,
};

use crate::puzzle::{self, Nono, Puzzle, Triano};

fn olsak_ch(c: char, orig_to_sanitized: &mut HashMap<char, char>) -> char {
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
    use crate::puzzle::Corner;
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
