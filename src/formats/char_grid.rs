use std::{
    char::from_digit,
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    iter::FromIterator,
};

use crate::puzzle::{ClueStyle, Color, ColorInfo, Corner, Solution, BACKGROUND};

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
            eprintln!(
                "number-loom: Warning: unable to guess which character is supposed to be the background; using the upper-left corner"
            );
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

pub fn as_char_grid(solution: &Solution) -> String {
    let mut result = String::new();

    for y in 0..solution.grid[0].len() {
        for x in 0..solution.grid.len() {
            let color = solution.grid[x][y];
            let color_info = &solution.palette[&color];
            result.push(color_info.ch);
        }
        result.push('\n');
    }
    result
}
