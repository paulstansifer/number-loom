//! Mirek and Petr Olsak's format (`.g`), which supports most every kind of puzzle.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    iter::FromIterator,
};

use anyhow::{Context, bail};
use typed_index_collections::TiVec;

use crate::geometry::{ClueSet, FamilyIdx, GridKind, LaneIdx, Shape, Square, Tri};
use crate::puzzle::{
    self, BACKGROUND, ClueStyle, Color, ColorInfo, Corner, DynPuzzle, Nono, Puzzle, Triano,
};

/// How each of Olsak's six data groups maps onto our lanes. The inverse of the reading done in
/// `olsak_triddler`; see that function for why the flags are what they are.
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
            c
        } else {
            for c in 'a'..='z' {
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
        for (name, family) in [("rows", FamilyIdx(0)), ("columns", FamilyIdx(1))] {
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

/// Assemble a triddler from Olsak's six data groups.
///
/// Olsak labels the hexagon's sides `A`..`F` counterclockwise from the upper left:
///
/// ```text
///          F
///       -------
///    A /       \ E
///     /        /
///     \       / D
///    B \     /
///       -----
///         C
/// ```
///
/// so `A`=topleft, `B`=bottomleft, `C`=bottom, `D`=bottomright, `E`=topright, `F`=top. Because the
/// traversal is counterclockwise, `A`/`B` and `C`/`D` list their lines in increasing lane order,
/// but `E`/`F` run the other way around the hexagon and so are listed in *decreasing* lane order.
///
/// The `C`/`D` blocks are written in the reverse of our order, which is what Olsak's own warning
/// about reading columns that "begin at the bottom of hexagonal ... from underneath upstairs"
/// refers to. (Note that these two facts were confirmed empirically: of the 1024 readings that fit
/// `tkocka.g`'s line lengths, only two solve it completely, and this is the one that also matches
/// the documented side diagram. The other is its mirror image.)
///
/// A blank line inside a group is significant — it means "no blocks in this line" — so unlike most
/// of this format, trailing blank lines must *not* be trimmed.
///
/// Olsak also documents two identities the side lengths must satisfy (`E = A + B - D` and
/// `F = C + D - A`); those hold automatically for any real outline, so rather than checking them
/// we just recover the outline from the six lengths and let that fail if they're inconsistent.
fn olsak_triddler(
    palette: HashMap<Color, ColorInfo>,
    mut groups: Vec<Vec<Vec<Nono>>>,
) -> anyhow::Result<Puzzle<Nono, Tri>> {
    use crate::geometry::{ClueSet, ClueSetCounts, Geometry, Outline};

    let counts = ClueSetCounts {
        topleft: groups[0].len(),
        bottomleft: groups[1].len(),
        bottom: groups[2].len(),
        bottomright: groups[3].len(),
        topright: groups[4].len(),
        top: groups[5].len(),
    };
    let outline = Outline::from_clue_set_counts(counts)?;
    let geometry = Geometry::<Tri>::new(outline);

    let mut lines: TiVec<LaneIdx, Vec<Nono>> = vec![vec![]; geometry.lane_map().lanes.len()].into();
    // Group index, its clue set, whether Olsak lists that side's lines backwards, and whether the
    // blocks within each line are written in the opposite order to ours.
    let assignment = [
        (0, ClueSet::TopLeft, false, false),
        (1, ClueSet::BottomLeft, false, false),
        (2, ClueSet::Bottom, false, true),
        (3, ClueSet::BottomRight, false, true),
        (4, ClueSet::TopRight, true, false),
        (5, ClueSet::Top, true, false),
    ];
    for (group_idx, clue_set, lines_reversed, blocks_reversed) in assignment {
        let mut group_lines = std::mem::take(&mut groups[group_idx]);
        if lines_reversed {
            group_lines.reverse();
        }
        for (lane, mut clue_line) in geometry
            .lanes_in_clue_set(clue_set)
            .into_iter()
            .zip(group_lines)
        {
            if blocks_reversed {
                clue_line.reverse();
            }
            lines[lane] = clue_line;
        }
    }

    Ok(Puzzle::triangular(palette, outline, lines))
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
    let mut olsak_glued_palettes = [
        HashMap::<(char, Glue), ColorInfo>::new(),
        HashMap::<(char, Glue), ColorInfo>::new(),
    ];
    let mut clue_style = ClueStyle::Nono;
    // `#t`/`#T` declares a triddler, which has six data groups rather than two.
    let mut triddler = false;

    // Dimension > Position > Clue index
    let mut nono_clues: Vec<Vec<Vec<Nono>>> = vec![vec![]; 6];
    let mut triano_clues: Vec<Vec<Vec<Triano>>> = vec![vec![], vec![]];

    let rrggbb = regex::Regex::new(r"^#(..)(..)(..)$").unwrap();
    let palette_line = regex::Regex::new(r"^\s*(\S):(.)\s+(\S+)\s*(.*)$").unwrap();

    for line in olsak.lines() {
        if let Some(palette_ch) = line.strip_prefix("#") {
            if cur_stanza != Preamble {
                bail!("Palette initiator (line beginning with '#') must be the first content");
            }

            let palette_ch = palette_ch.to_lowercase();

            // `#t`/`#T` only declares that this is a triddler; the palette (if any) is still
            // introduced by a separate `#d`, and comments may sit between the two. A triddler
            // with no colors has no `#d` at all.
            if palette_ch.starts_with("t") {
                triddler = true;
            } else if palette_ch.starts_with("d") {
                cur_stanza = Palette;
            } else {
                bail!("unrecognized directive: #{palette_ch}");
            }
        } else if line.starts_with(":") {
            cur_stanza = Dimension(if let Dimension(n) = cur_stanza {
                n + 1
            } else {
                0
            });
        } else if cur_stanza == Preamble {
            /* Just comments */
        } else if cur_stanza == Palette {
            if line.trim().is_empty() {
                continue;
            }
            let captures = palette_line
                .captures(line)
                .ok_or(anyhow::anyhow!("Malformed palette line {line}"))?;

            let (_, [input_ch, unique_ch, color_name, comment]) = captures.extract();

            let parse_glue = |c| match c {
                '>' => Right,
                '<' => Left,
                _ => NoGlue,
            };

            let rising = color_name.contains('/');

            let (corner, unique_ch) = match (color_name.split_once(['/', '\\']), rising) {
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

            let rgb =
                if let Some((_, [rs, gs, bs])) = rrggbb.captures(color_name).map(|c| c.extract()) {
                    (
                        u8::from_str_radix(rs, 16).context("expected hex digits in color")?,
                        u8::from_str_radix(gs, 16).context("expected hex digits in color")?,
                        u8::from_str_radix(bs, 16).context("expected hex digits in color")?,
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

            let dim_0_glue = comment.chars().next().map(parse_glue).unwrap_or(NoGlue);
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
            olsak_palette.entry('1').or_insert_with(|| ColorInfo {
                ch: '#',
                name: "black".to_string(),
                rgb: (0, 0, 0),
                color: Color(next_color),
                corner: None,
            });

            if d >= if triddler { 6 } else { 2 } {
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
                            let color = olsak_palette
                                .get(&input_ch)
                                .with_context(|| format!("undefined color: {input_ch}"))?
                                .color;
                            clues.push(Nono {
                                color,
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
                        let front_cap = chars.first().and_then(|c| {
                            olsak_glued_palettes[d].get(&(*c, Left)).map(|c| c.color)
                        });
                        if front_cap.is_some() {
                            chars.remove(0);
                        }
                        let back_cap = chars.last().and_then(|c| {
                            olsak_glued_palettes[d].get(&(*c, Right)).map(|c| c.color)
                        });
                        if back_cap.is_some() {
                            chars.pop();
                        }
                        let last_char = *chars.last().context("clue has no body")?;
                        let body_color = if !last_char.is_numeric() {
                            let body_ch = chars.pop().unwrap();
                            olsak_palette
                                .get(&body_ch)
                                .with_context(|| format!("undefined color: {body_ch}"))?
                                .color
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
    olsak_palette
        .entry('0')
        .or_insert_with(ColorInfo::default_bg);

    let mut palette: HashMap<Color, ColorInfo> = olsak_palette
        .into_values()
        .map(|ci| (ci.color, ci))
        .collect();
    for glued_palette in olsak_glued_palettes {
        for (_, ci) in glued_palette.iter() {
            palette.insert(ci.color, ci.clone());
        }
    }

    if triddler {
        if clue_style == ClueStyle::Triano {
            bail!("a puzzle can't be both a triddler and a trianogram");
        }
        return Ok(olsak_triddler(palette, nono_clues)?.into());
    }

    Ok(match clue_style {
        ClueStyle::Nono => {
            Puzzle::<Nono, Square>::square(palette, nono_clues[0].clone(), nono_clues[1].clone())
                .into()
        }
        ClueStyle::Triano => Puzzle::<Triano, Square>::square(
            palette,
            triano_clues[0].clone(),
            triano_clues[1].clone(),
        )
        .into(),
    })
}

#[cfg(test)]
mod triddler_tests {
    use super::olsak_to_puzzle;
    use crate::geometry::{Geometry, Outline, Tri};
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
            cells.into(),
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
        // Olsak renumbers color indices on the way through, so compare by RGB (the existing
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

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, iter::FromIterator};

    use anyhow::bail;

    use super::olsak_to_puzzle;
    use crate::{
        geometry::Square,
        puzzle::{Color, ColorInfo, Corner, Puzzle, Triano},
    };

    fn match_march<'a, T>(
        lhs: &'a [T],
        rhs: &'a [T],
    ) -> anyhow::Result<Box<dyn Iterator<Item = (&'a T, &'a T)> + 'a>> {
        if lhs.len() != rhs.len() {
            anyhow::bail!("Length mismatch: {} vs {}", lhs.len(), rhs.len());
        }
        Ok(Box::new(lhs.iter().zip(rhs.iter())))
    }

    fn colors_eq(
        lhs: Color,
        rhs: Color,
        lhs_pal: &HashMap<Color, ColorInfo>,
        rhs_pal: &HashMap<Color, ColorInfo>,
    ) -> anyhow::Result<()> {
        if lhs_pal[&lhs].rgb != rhs_pal[&rhs].rgb {
            bail!(
                "Color mismatch: {:?} vs {:?}",
                lhs_pal[&lhs].rgb,
                rhs_pal[&rhs].rgb
            );
        }
        if lhs_pal[&lhs].corner != rhs_pal[&rhs].corner {
            bail!("corner mismatch");
        }
        Ok(())
    }

    fn puzzles_eq(
        lhs: &Puzzle<Triano, Square>,
        rhs: &Puzzle<Triano, Square>,
    ) -> anyhow::Result<()> {
        if lhs.row_clues().len() != rhs.row_clues().len() {
            bail!(
                "Row length mismatch {} vs {}",
                lhs.row_clues().len(),
                rhs.row_clues().len()
            );
        }

        for (l_lines, r_lines, _dim) in [
            (lhs.col_clues(), rhs.col_clues(), "col"),
            (lhs.row_clues(), rhs.row_clues(), "row"),
        ] {
            for (l_row, r_row) in match_march(&l_lines.raw, &r_lines.raw)? {
                for (l_clue, r_clue) in match_march(l_row, r_row)? {
                    if let (Some(l), Some(r)) = (l_clue.front_cap, r_clue.front_cap) {
                        colors_eq(l, r, &lhs.palette, &rhs.palette)?;
                    } else {
                        if l_clue.front_cap.is_some() != r_clue.front_cap.is_some() {
                            bail!("front cap mismatch");
                        }
                    }
                    colors_eq(
                        l_clue.body_color,
                        r_clue.body_color,
                        &lhs.palette,
                        &rhs.palette,
                    )?;
                    if l_clue.body_len != r_clue.body_len {
                        bail!(
                            "body length mismatch: {} vs {}",
                            l_clue.body_len,
                            r_clue.body_len
                        );
                    }

                    if let (Some(l), Some(r)) = (l_clue.back_cap, r_clue.back_cap) {
                        colors_eq(l, r, &lhs.palette, &rhs.palette)?;
                    } else {
                        if l_clue.back_cap.is_some() != r_clue.back_cap.is_some() {
                            bail!("front cap mismatch");
                        }
                    }
                }
            }
        }

        Ok(())
    }

    #[test]
    fn round_trip_olsak_triano() {
        let palette = HashMap::from_iter([
            (Color(0), ColorInfo::default_bg()),
            (Color(1), ColorInfo::default_fg(Color(1))),
            (
                Color(2),
                ColorInfo {
                    ch: '◢',
                    name: "foo".to_string(),
                    rgb: (0, 0, 0),
                    color: Color(2),
                    corner: Some(Corner {
                        upper: false,
                        left: false,
                    }),
                },
            ),
        ]);
        // Listen: I know this isn't a coherent puzzle
        let cols = vec![vec![
            Triano {
                front_cap: Some(Color(2)),
                body_len: 3,
                body_color: Color(1),
                back_cap: None,
            },
            Triano {
                front_cap: None,
                body_len: 2,
                body_color: Color(1),
                back_cap: None,
            },
        ]];
        let rows = vec![vec![Triano {
            front_cap: None,
            body_len: 3,
            body_color: Color(1),
            back_cap: None,
        }]];

        let p = Puzzle::<Triano, Square>::square(palette, rows, cols);

        let serialized = super::as_olsak_triano(&p);

        println!("{}", serialized);

        let roundtripped = olsak_to_puzzle(&serialized).unwrap();

        println!("{:?}", roundtripped);

        puzzles_eq(&p, &roundtripped.as_square_triano().unwrap()).unwrap();
    }
}
