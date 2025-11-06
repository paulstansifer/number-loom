use std::path::PathBuf;


use crate::{
    formats,
    puzzle::{self, Document, NonogramFormat},
};

pub fn to_bytes(
    document: &mut Document,
    file_name: Option<String>,
    format: Option<NonogramFormat>,
) -> anyhow::Result<Vec<u8>> {
    let format = format.unwrap_or_else(|| {
        puzzle::infer_format(
            file_name
                .as_ref()
                .expect("gotta have SOME clue about format"),
            None,
        )
    });

    let bytes = if format == NonogramFormat::Image {
        let file_name = file_name.expect("need file name to pick image format");
        formats::image::as_image_bytes(document.solution()?, file_name)?
    } else {
        match format {
            NonogramFormat::Olsak => document
                .puzzle()
                .specialize(formats::olsak::as_olsak_nono, formats::olsak::as_olsak_triano),
            NonogramFormat::Webpbn => formats::webpbn::as_webpbn(document),
            NonogramFormat::Html => document.puzzle().specialize(formats::html::as_html, formats::html::as_html),
            NonogramFormat::Image => panic!(),
            NonogramFormat::CharGrid => formats::char_grid::as_char_grid(document.solution()?),
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

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, iter::FromIterator};

    use anyhow::bail;

    use crate::{
        formats,
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

    fn puzzles_eq(lhs: &Puzzle<Triano>, rhs: &Puzzle<Triano>) -> anyhow::Result<()> {
        if lhs.rows.len() != rhs.rows.len() {
            bail!(
                "Row length mismatch {} vs {}",
                lhs.rows.len(),
                rhs.rows.len()
            );
        }

        for (l_lines, r_lines, _dim) in
            [(&lhs.cols, &rhs.cols, "col"), (&lhs.rows, &rhs.rows, "row")]
        {
            for (l_row, r_row) in match_march(&l_lines, &r_lines)? {
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
        let p = Puzzle::<Triano> {
            palette: HashMap::from_iter([
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
            ]),
            // Listen: I know this isn't a coherent puzzle
            cols: vec![vec![
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
            ]],
            rows: vec![vec![Triano {
                front_cap: None,
                body_len: 3,
                body_color: Color(1),
                back_cap: None,
            }]],
        };

        let serialized = formats::olsak::as_olsak_triano(&p);

        println!("{}", serialized);

        let roundtripped = formats::olsak::olsak_to_puzzle(&serialized).unwrap();

        println!("{:?}", roundtripped);

        puzzles_eq(&p, &roundtripped.assume_triano()).unwrap();
    }
}
