//! This is used both by webpbn's format and WOVEN for representing partial solutions.
//! `?` means a cell that could be anything, `[abc]` means a cell that could be any of `a`, `b`, or `c`.
//! Everything other than `?`, `[`, and `]` is an entry in the color palette.

use std::collections::HashMap;

use anyhow::{Context, bail};

use crate::puzzle::{Color, UNSOLVED};
use crate::solve::line_solve::Cell;

const UNKNOWN: char = '?';
const OPEN: char = '[';
const CLOSE: char = ']';

/// No color can be spelled with one of these, or the notation would be ambiguous.
pub const RESERVED_CHS: [char; 3] = [UNKNOWN, OPEN, CLOSE];

pub fn parse_cells(text: &str, ch_to_color: &HashMap<char, Color>) -> anyhow::Result<Vec<Cell>> {
    let color_of = |ch: char| {
        ch_to_color
            .get(&ch)
            .copied()
            .with_context(|| format!("undefined color char: {ch:?}"))
    };

    let mut cells = vec![];
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        cells.push(match ch {
            UNKNOWN => Cell::new_anything(),
            OPEN => {
                let mut cell = Cell::new_impossible();
                loop {
                    match chars.next() {
                        Some(CLOSE) => break,
                        Some(ch) => cell.actually_could_be(color_of(ch)?),
                        None => bail!("unclosed {OPEN:?}"),
                    }
                }
                cell
            }
            ch => Cell::from_color(color_of(ch)?),
        });
    }
    Ok(cells)
}

/// The inverse of `parse_cells`, or `None` if some cell's color has no `ch`. `?` is used for
/// any cell that could be every color in `ch_of`.
pub fn spell_cells(cells: &[Cell], ch_of: &HashMap<Color, char>) -> Option<String> {
    let palette = cell_colors(ch_of.keys().copied());

    let mut res = String::new();
    for cell in cells {
        if let Some(color) = cell.known_or() {
            res.push(*ch_of.get(&color)?);
        } else if could_be_anything(cell, &palette) {
            res.push(UNKNOWN);
        } else {
            res.push(OPEN);
            // Possibilities outside the palette can't happen, so there's no need to write them.
            res.extend(
                palette
                    .iter()
                    .filter(|&&color| cell.can_be(color))
                    .map(|color| ch_of[color]),
            );
            res.push(CLOSE);
        }
    }
    Some(res)
}

/// Whether anything at all is known about `cells`, given the colors in the palette: if not,
/// `spell_cells` would write nothing but `?`, and there's no point saving them.
pub fn has_progress(cells: &[Cell], palette: impl IntoIterator<Item = Color>) -> bool {
    let palette = cell_colors(palette);
    cells
        .iter()
        .any(|cell| cell.is_known() || !could_be_anything(cell, &palette))
}

/// The palette colors a `Cell` can talk about, in order.
fn cell_colors(palette: impl IntoIterator<Item = Color>) -> Vec<Color> {
    // `UNSOLVED` may be in a palette, but it's too big to fit in a `Cell`'s mask.
    let mut res: Vec<Color> = palette.into_iter().filter(|&c| c != UNSOLVED).collect();
    res.sort();
    res
}

fn could_be_anything(cell: &Cell, palette: &[Color]) -> bool {
    palette.iter().all(|&color| cell.can_be(color))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::puzzle::BACKGROUND;

    #[test]
    fn round_trip() {
        let (b, x, r) = (BACKGROUND, Color(1), Color(2));
        let ch_of: HashMap<Color, char> = [(b, '.'), (x, 'X'), (r, 'r')].into();
        let ch_to_color = ch_of.iter().map(|(&c, &ch)| (ch, c)).collect();

        let mut two_ways = Cell::new_impossible();
        two_ways.actually_could_be(b);
        two_ways.actually_could_be(r);

        let cells = vec![
            Cell::from_color(x),
            Cell::new_anything(),
            two_ways,
            Cell::new_impossible(),
        ];
        let text = spell_cells(&cells, &ch_of).unwrap();
        assert_eq!(text, "X?[.r][]");
        assert_eq!(parse_cells(&text, &ch_to_color).unwrap(), cells);

        // Every palette color, but not spelled `?`, still means the same thing.
        let every_color = parse_cells("[.Xr]", &ch_to_color).unwrap();
        assert_eq!(spell_cells(&every_color, &ch_of).unwrap(), "?");
    }

    #[test]
    fn progress_is_anything_short_of_every_color() {
        let (b, x) = (BACKGROUND, Color(1));
        let mut either = Cell::new_impossible();
        either.actually_could_be(b);
        either.actually_could_be(x);

        assert!(!has_progress(&[Cell::new_anything(), either], [b, x]));
        assert!(has_progress(
            &[Cell::new_anything(), either],
            [b, x, Color(2)]
        ));
        assert!(has_progress(&[Cell::from_color(b)], [b]));
        assert!(!has_progress(&[], [b, x]));
    }

    #[test]
    fn rejects_malformed_text() {
        let ch_to_color: HashMap<char, Color> = [('.', BACKGROUND)].into();
        assert!(parse_cells("[.", &ch_to_color).is_err());
        assert!(parse_cells("Q", &ch_to_color).is_err());
        assert!(parse_cells("[Q]", &ch_to_color).is_err());
    }
}
