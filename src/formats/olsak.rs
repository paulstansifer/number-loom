use std::{
    collections::{HashMap, HashSet},
    iter::FromIterator,
};

use crate::geometry::{ClueSet, GridKind, Shape, Square};
use crate::puzzle::{self, Nono, Puzzle, Triano};

/// How each of Olsak's six data groups maps onto our lanes. The inverse of the reading done in
/// `import::olsak_triddler`; see that function for why the flags are what they are.
const OLSAK_TRIDDLER_GROUPS: [(ClueSet, bool, bool); 6] = [
    (ClueSet::TopLeft, false, false),
    (ClueSet::BottomLeft, false, false),
    (ClueSet::Bottom, false, true),
    (ClueSet::BottomRight, false, true),
    (ClueSet::TopRight, true, false),
    (ClueSet::Top, true, false),
];

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

pub fn as_olsak_nono<K: GridKind>(puzzle: &Puzzle<Nono, K>) -> String {
    let mut orig_to_sanitized: HashMap<char, char> = HashMap::new();

    let mut palette = puzzle.palette.clone();

    let triangular = matches!(puzzle.geometry.shape(), Shape::Triangular(_));

    let mut res = String::new();
    if triangular {
        // Declares a triddler; the palette still gets its own `#d`.
        res.push_str("#t\n");
    }
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
    let write_line = |res: &mut String, clues: &[Nono], reversed: bool| {
        let mut clues: Vec<&Nono> = clues.iter().collect();
        if reversed {
            clues.reverse();
        }
        for clue in clues {
            res.push_str(&format!("{}{} ", clue.count, palette[&clue.color].ch));
        }
        res.push('\n');
    };

    if triangular {
        for (clue_set, lines_reversed, blocks_reversed) in OLSAK_TRIDDLER_GROUPS {
            res.push_str(&format!(": {:?}\n", clue_set));
            let mut lanes = puzzle.geometry.lanes_in_clue_set(clue_set);
            if lines_reversed {
                lanes.reverse();
            }
            for lane in lanes {
                write_line(&mut res, &puzzle.lines[lane], blocks_reversed);
            }
        }
    } else {
        // Family 0 is rows and family 1 is columns, which is what this format calls them.
        for (name, family) in [("rows", 0), ("columns", 1)] {
            res.push_str(&format!(": {name}\n"));
            for lane in puzzle.lane_map().family(family) {
                write_line(&mut res, &puzzle.lines[lane], false);
            }
        }
    }

    res
}

pub fn as_olsak_triano(puzzle: &Puzzle<Triano, Square>) -> String {
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
    for row in puzzle.row_clues() {
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
    for column in puzzle.col_clues() {
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

#[cfg(test)]
mod triddler_tests {
    use crate::geometry::{Geometry, Outline, Tri};
    use crate::import::olsak_to_puzzle;
    use crate::puzzle::{BACKGROUND, ClueStyle, Color, Solution};

    /// Build a triddler from a picture, write it as olsak, read it back, and check we get the
    /// same clues on the same shape.
    fn round_trip(outline: Outline, fill: impl Fn(usize) -> bool) {
        let geometry = Geometry::<Tri>::new(outline);
        let cells: Vec<Color> = (0..geometry.cell_count())
            .map(|i| if fill(i) { Color(1) } else { BACKGROUND })
            .collect();
        let solution = Solution::new(
            ClueStyle::Nono,
            crate::import::bw_palette(),
            geometry,
            cells,
        );

        let original = solution.to_puzzle();
        let serialized = super::as_olsak_nono(original.as_tri_nono().unwrap());
        assert!(serialized.starts_with("#t\n"), "must declare a triddler");

        let reloaded = olsak_to_puzzle(&serialized).expect("should re-read");

        assert_eq!(
            original.as_tri_nono().unwrap().geometry,
            reloaded.as_tri_nono().unwrap().geometry,
            "same shape, including position (outlines are canonicalized)"
        );
        // Olsak renumbers colour indices on the way through, so compare by RGB (the existing
        // square round-trip test does the same).
        let as_rgb =
            |p: &crate::puzzle::Puzzle<crate::puzzle::Nono, Tri>| -> Vec<Vec<(u16, (u8, u8, u8))>> {
                p.lines
                    .iter()
                    .map(|line| {
                        line.iter()
                            .map(|clue| (clue.count, p.palette[&clue.color].rgb))
                            .collect()
                    })
                    .collect()
            };
        assert_eq!(
            as_rgb(original.as_tri_nono().unwrap()),
            as_rgb(reloaded.as_tri_nono().unwrap())
        );
    }

    #[test]
    fn triddlers_round_trip_through_olsak() {
        for side in 1..=4 {
            round_trip(Outline::hexagon(side), |i| i % 3 != 0);
            round_trip(Outline::hexagon(side), |i| i % 5 < 2);
            // An entirely empty lane exercises the blank-line convention, which is significant
            // in this format.
            round_trip(Outline::hexagon(side), |_| false);
        }
        // An off-centre outline, with a sharp corner or two.
        round_trip(
            Outline {
                a: (0, 2),
                b: (1, 3),
                c: (-1, 2),
            },
            |i| i % 4 != 1,
        );
    }
}
