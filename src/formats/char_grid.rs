//! Plain text, interpreted as a bitmap, sort of. Just guess the palette!
//!
//! 
//! A grid whose first non-whitespace character is `/` is read as a triddler

use std::{
    char::from_digit,
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    iter::FromIterator,
};

use anyhow::{Context, bail};
use typed_index_collections::TiVec;

use crate::{
    geometry::{CellIdx, Square, Tri},
    puzzle::{BACKGROUND, ClueStyle, Color, ColorInfo, Corner, DynSolution, Solution},
};

/// Assigns a `ColorInfo` (background, then black-ish, then everything else) to each character a
/// chargrid actually uses. Shared between the square and triddler readers below; `fallback_bg` is
/// only consulted if nothing on the guess list turns up, and should be some character the caller
/// knows is actually in the grid.
fn build_char_palette(
    mut unused_chars: BTreeSet<char>,
    fallback_bg: char,
) -> HashMap<char, ColorInfo> {
    let mut palette = HashMap::<char, ColorInfo>::new();

    // the characters this matters for are in ASCII
    let any_uppercase = unused_chars.iter().any(|ch| ch.is_ascii_uppercase());

    let mut bg_ch: Option<char> = None;

    // Look for a character that seems to represent a white background.
    for possible_bg in [' ', '.', '_', 'w', 'W', '·', '☐', '0', 'x', '⬜'] {
        if unused_chars.contains(&possible_bg) {
            bg_ch = Some(possible_bg);
        }
    }

    // But we need to *some* color as background to proceed!
    let bg_ch = match bg_ch {
        Some(x) => x,
        None => {
            eprintln!(
                "number-loom: Warning: unable to guess which character is supposed to be the background; using the upper-left corner"
            );
            fallback_bg
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
    for possible_black in ['#', '.', '■', '█', '1', '⬛', 'B', 'b'] {
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
    if any_uppercase {
        unused_colors.insert('R', (255, 0, 0));
        unused_colors.insert('G', (0, 255, 0));
        unused_colors.insert('B', (0, 0, 255));

        unused_colors.insert('Y', (255, 255, 0));
        unused_colors.insert('C', (0, 255, 255));
        unused_colors.insert('M', (255, 0, 255));
    } else {
        unused_colors.insert('r', (255, 0, 0));
        unused_colors.insert('g', (0, 255, 0));
        unused_colors.insert('b', (0, 0, 255));

        unused_colors.insert('y', (255, 255, 0));
        unused_colors.insert('c', (0, 255, 255));
        unused_colors.insert('m', (255, 0, 255));
    }
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

    palette
}

/// A chargrid whose first non-whitespace character is `/` is a triddler (see
/// `char_grid_to_tri_solution`); otherwise `/` and `\` are just ordinary color characters, as
/// they've always been.
pub fn char_grid_to_solution(char_grid: &str) -> anyhow::Result<DynSolution> {
    if char_grid.trim_start().starts_with('/') {
        return Ok(DynSolution::Tri(char_grid_to_tri_solution(char_grid)?));
    }
    Ok(DynSolution::Square(char_grid_to_square_solution(char_grid)))
}

fn char_grid_to_square_solution(char_grid: &str) -> Solution<Square> {
    // We want deterministic behavior
    let mut unused_chars = BTreeSet::<char>::new();
    for ch in char_grid.chars() {
        if ch == '\n' {
            continue;
        }
        unused_chars.insert(ch);
    }
    let fallback_bg = char_grid.trim_start().chars().next().unwrap();
    let mut palette = build_char_palette(unused_chars, fallback_bg);

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
        for color_info in palette.values_mut() {
            if color_info.color == BACKGROUND {
                continue;
            }
            color_info.rgb = (0, 0, 0);
        }

        ClueStyle::Triano
    } else {
        ClueStyle::Nono
    };

    Solution::from_columns(
        clue_style,
        palette
            .into_values()
            .map(|color_info| (color_info.color, color_info))
            .collect(),
        grid,
    )
}

/// Reads a triddler drawn the way `webpbn_tridder.md`'s `<solution>` does: one text row per `a`
/// (the horizontal-row family), each cell one character wide, with `/` or `\` at either end
/// marking where the row's edge slopes (see `webpbn::parse_solution_image` for the same
/// convention on webpbn's own solution images). Unlike that reader, we don't already know the
/// outline from clue counts, so we recover it from where the characters actually land: a
/// character's column, read from the start of its (unindented) line, is exactly a cell's
/// half-unit position `p` in `TriCoord::from_row_and_half_unit(a, p)` — indentation is what lines
/// each row's cells up against its neighbors, so it must be read verbatim rather than trimmed.
fn char_grid_to_tri_solution(char_grid: &str) -> anyhow::Result<Solution<Tri>> {
    use crate::geometry::{Geometry, Outline, TriCoord};

    let mut unused_chars = BTreeSet::<char>::new();
    let mut fallback_bg = None;
    let mut ch_by_coord = HashMap::<TriCoord, char>::new();

    for (row, line) in char_grid
        .lines()
        .filter(|line| !line.trim().is_empty())
        .enumerate()
    {
        for (col, ch) in line.chars().enumerate() {
            if ch.is_whitespace() || ch == '/' || ch == '\\' {
                continue;
            }
            unused_chars.insert(ch);
            fallback_bg.get_or_insert(ch);

            let coord = TriCoord::from_row_and_half_unit(row as i32, col as i32);
            if ch_by_coord.insert(coord, ch).is_some() {
                bail!("two characters in the triddler chargrid landed on the same cell");
            }
        }
    }

    let fallback_bg = fallback_bg.context("triddler chargrid has no cells")?;
    let palette = build_char_palette(unused_chars, fallback_bg);

    // The outline is exactly the box these coordinates span: a real triddler's rows are convex,
    // so (unlike an arbitrary blob of characters) the box this implies can't include any cell we
    // didn't see — the fill-in loop below still checks, in case the grid wasn't actually convex.
    let widen = |range: Option<(i32, i32)>, v: i32| match range {
        None => Some((v, v)),
        Some((lo, hi)) => Some((lo.min(v), hi.max(v))),
    };
    let (mut a_range, mut b_range, mut c_range) = (None, None, None);
    for coord in ch_by_coord.keys() {
        a_range = widen(a_range, coord.a);
        b_range = widen(b_range, coord.b);
        c_range = widen(c_range, coord.c);
    }
    let outline = Outline {
        a: a_range.context("triddler chargrid has no cells")?,
        b: b_range.unwrap(),
        c: c_range.unwrap(),
    };
    let geometry = Geometry::<Tri>::new(outline);

    let mut cells: TiVec<CellIdx, Color> = vec![BACKGROUND; geometry.cell_count()].into();
    for i in geometry.lane_map.cells() {
        let coord = geometry.coords[i];
        let ch = ch_by_coord
            .get(&coord)
            .with_context(|| format!("triddler chargrid is missing a cell at {coord:?}"))?;
        cells[i] = palette[ch].color;
    }

    Ok(Solution::new(
        ClueStyle::Nono, // webpbn/triddler chargrids can't represent trianogram clues
        palette
            .into_values()
            .map(|color_info| (color_info.color, color_info))
            .collect(),
        geometry,
        cells,
    ))
}

pub fn as_char_grid(solution: &Solution<Square>) -> String {
    let mut result = String::new();

    for y in 0..solution.y_size() {
        for x in 0..solution.x_size() {
            let color = solution[(x, y)];
            let color_info = &solution.palette[&color];
            result.push(color_info.ch);
        }
        result.push('\n');
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::FamilyIdx;

    #[test]
    fn a_leading_slash_after_whitespace_still_means_triddler() {
        // A single row is trivially a valid outline on its own (its two ends are the whole
        // shape's extremes in every direction), so this also exercises the reader end to end.
        let solution = match char_grid_to_solution("\n  /XY\\\n").unwrap() {
            DynSolution::Tri(s) => s,
            DynSolution::Square(_) => panic!("expected a triddler"),
        };
        assert_eq!(solution.geometry.cell_count(), 2);
    }

    #[test]
    fn slashes_are_ordinary_colors_when_the_grid_does_not_start_with_one() {
        // Same characters as above, just not leading — so `/` and `\` are colors, as they've
        // always been for chargrids.
        assert!(matches!(
            char_grid_to_solution(".X/\n\\X.\n").unwrap(),
            DynSolution::Square(_)
        ));
    }

    /// The worked example from `webpbn_tridder.md` (see also `webpbn::tests::DOC_TRIDDLER`),
    /// stored the way its `<solution>` would be: one text row per `a`, cells packed one character
    /// wide, `/`/`\` marking each row's slope. Its outline — rows of 5, 6, 5 cells, 16 total — is
    /// independently pinned down in `webpbn::tests::reads_the_doc_triddler` from the clue counts,
    /// so matching it here confirms the chargrid reader recovers the same shape from geometry
    /// alone.
    #[test]
    fn reads_the_doc_triddler_shape_from_a_chargrid() {
        let grid = "         /ABCDE\\\n        /FGHIJK/\n        \\LMNOP/\n";
        let solution = match char_grid_to_solution(grid).unwrap() {
            DynSolution::Tri(s) => s,
            DynSolution::Square(_) => panic!("expected a triddler"),
        };

        assert_eq!(solution.geometry.cell_count(), 16);
        let rows: Vec<usize> = solution
            .geometry
            .family(FamilyIdx(0))
            .map(|i| solution.geometry.lane_map.lanes[i].cells.len())
            .collect();
        assert_eq!(rows, vec![5, 6, 5]);

        // 16 distinct letters, none repeated, should mean 16 distinct colors — i.e. every
        // character landed on its own cell rather than colliding with a neighbor.
        assert_eq!(solution.palette.len(), 16);
    }

    /// The top two rows of the doc-triddler example, at the exact columns
    /// `reads_the_doc_triddler_shape_from_a_chargrid` uses, but with every cell in a row sharing
    /// one character — a truncated hexagon is still a valid (if lopsided) outline, and repeating
    /// a character across an entire row only works if every one of that row's characters lines up
    /// on its own cell rather than colliding.
    #[test]
    fn repeated_characters_share_one_color() {
        let grid = "         /XXXXX\\\n        /YYYYYY/\n";
        let solution = match char_grid_to_solution(grid).unwrap() {
            DynSolution::Tri(s) => s,
            DynSolution::Square(_) => panic!("expected a triddler"),
        };

        assert_eq!(solution.geometry.cell_count(), 11); // 5 + 6, as in the full example.
        let rows: Vec<usize> = solution
            .geometry
            .family(FamilyIdx(0))
            .map(|i| solution.geometry.lane_map.lanes[i].cells.len())
            .collect();
        assert_eq!(rows, vec![5, 6]);
        // Only two distinct letters were used, so only two colors should have been minted.
        assert_eq!(solution.palette.len(), 2);
    }
}
