use anyhow::Context;
use std::path::{Path, PathBuf};

use image::{DynamicImage, ImageFormat, Rgb, RgbImage};

use crate::{
    formats::html::as_html,
    formats::woven::to_woven,
    geometry::{Shape, Square},
    puzzle::{self, Document, DynPuzzle, NonogramFormat, Solution},
};

/// The square-only writers need a square picture; asking for one is how we find out.
fn square_solution(document: &mut Document) -> anyhow::Result<&Solution<Square>> {
    document
        .solution()?
        .as_square()
        .context("this format needs a square puzzle, not a triddler")
}

pub fn to_bytes(
    document: &mut Document,
    file_name: Option<String>,
    format: Option<NonogramFormat>,
) -> anyhow::Result<Vec<u8>> {
    use crate::formats::olsak::{as_olsak_nono, as_olsak_triano};
    use crate::formats::webpbn::as_webpbn;
    let format = format.unwrap_or_else(|| {
        puzzle::infer_format(
            file_name
                .as_ref()
                .expect("gotta have SOME clue about format"),
            None,
        )
    });

    // Triangular puzzles round-trip through webpbn and olsak, and draw as HTML. The other
    // writers all assume two clue directions and a rectangular grid of cells, and would quietly
    // emit nonsense.
    if let Some(puzzle) = document.try_puzzle() {
        let triangular = matches!(puzzle.shape(), Shape::Triangular(_));
        let supports_triddlers = matches!(
            format,
            NonogramFormat::Webpbn | NonogramFormat::Olsak | NonogramFormat::Html
        );
        if triangular && !supports_triddlers {
            anyhow::bail!(
                "{:?} can't represent a triddler; use the webpbn, olsak or html format",
                format
            );
        }
    }

    let bytes = if format == NonogramFormat::Image {
        let file_name = file_name.expect("need file name to pick image format");
        as_image_bytes(square_solution(document)?, file_name)?
    } else {
        match format {
            NonogramFormat::Olsak => match document.puzzle() {
                DynPuzzle::SquareNono(p) => as_olsak_nono(p),
                DynPuzzle::TriNono(p) => as_olsak_nono(p),
                DynPuzzle::SquareTriano(p) => as_olsak_triano(p),
            },
            NonogramFormat::Webpbn => as_webpbn(document),
            NonogramFormat::Html => {
                let (title, author) = (document.title.clone(), document.author.clone());
                crate::with_puzzle!(document.puzzle(), |p| as_html(p, &title, &author))
            }
            NonogramFormat::Image => panic!(),
            NonogramFormat::Woven => to_woven(document)?,
            NonogramFormat::CharGrid => as_char_grid(square_solution(document)?),
        }
        .into_bytes()
    };

    Ok(bytes)
}

pub fn save(
    document: &mut Document,
    path: &PathBuf,
    format: Option<NonogramFormat>,
) -> anyhow::Result<()> {
    let bytes = to_bytes(document, Some(path.to_str().unwrap().to_string()), format)?;

    if path == &PathBuf::from("-") {
        use std::io::Write;
        std::io::stdout().write_all(&bytes)?;
        std::io::stdout().flush()?;
    } else {
        std::fs::write(path, bytes)?
    }
    Ok(())
}

pub fn as_image_bytes<P>(
    solution: &Solution<Square>,
    path_or_filename: P,
) -> anyhow::Result<Vec<u8>>
where
    P: AsRef<Path>,
{
    let mut image = RgbImage::new(solution.x_size() as u32, solution.y_size() as u32);

    for x in 0..solution.x_size() {
        for y in 0..solution.y_size() {
            let (r, g, b) = solution.palette[&solution[(x, y)]].rgb;
            image.put_pixel(x as u32, y as u32, Rgb::<u8>([r, g, b]));
        }
    }

    let image_format = ImageFormat::from_path(path_or_filename)?;

    let dyn_image: DynamicImage = image::DynamicImage::ImageRgb8(image);

    let mut writer = std::io::BufWriter::new(std::io::Cursor::new(Vec::new()));

    dyn_image.write_to(&mut writer, image_format)?;

    Ok(writer
        .into_inner()
        .expect("Couldn't get inner Vec<u8> from BufWriter")
        .into_inner())
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
    use std::{collections::HashMap, iter::FromIterator};

    use anyhow::bail;

    use crate::{
        geometry::Square,
        import::olsak_to_puzzle,
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
            for (l_row, r_row) in match_march(l_lines, r_lines)? {
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

        let serialized = crate::formats::olsak::as_olsak_triano(&p);

        println!("{}", serialized);

        let roundtripped = olsak_to_puzzle(&serialized).unwrap();

        println!("{:?}", roundtripped);

        puzzles_eq(&p, &roundtripped.as_square_triano().unwrap()).unwrap();
    }
}
