use std::{
    collections::{HashMap, HashSet},
    iter::FromIterator,
    path::{Path, PathBuf},
};

use axohtml::{html, text};
use image::{DynamicImage, ImageFormat, Rgb, RgbImage};

use crate::puzzle::{self, Clue, Document, Nono, NonogramFormat, Puzzle, Solution, Triano};

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
        as_image_bytes(document.solution()?, file_name)?
    } else {
        match format {
            NonogramFormat::Olsak => document.puzzle().specialize(as_olsak_nono, as_olsak_triano),
            NonogramFormat::Webpbn => as_webpbn(document),
            NonogramFormat::Html => document.puzzle().specialize(as_html, as_html),
            NonogramFormat::Image => panic!(),
            NonogramFormat::CharGrid => as_char_grid(document.solution()?),
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

pub fn as_html<C: Clue>(puzzle: &Puzzle<C>) -> String {
    let html: axohtml::dom::DOMTree<String> = html!(
        <html>
            <head>
            <title></title>
            <style>
            {text!(
"
table, td, th {
    border-collapse: collapse;
}
td {
    border: 1px solid black;
    width: 40px;
    height: 40px;
}

table tr:nth-of-type(5n) td {
    border-bottom: 3px solid;
}
table td:nth-of-type(5n) {
    border-right: 3px solid;
}

table tr:last-child td {
    border-bottom: 1px solid;
}
table td:last-child {
    border-right: 1px solid;
}
.col {
  vertical-align: bottom;
  border-top: none;
  font-family: courier;
}
.row {
  text-align: right;
  border-left: none;
  font-family: courier;
  padding-right: 6px;
}


    ")}
            </style>
            </head>
            <body>
                <table>
                    <thead>
                        <tr>
                        <th></th>
                        { puzzle.cols.iter().map(|col| html!(<th class="col">{
                            col.iter().map(|clue| html!(<div style=(clue.html_color(puzzle))>{text!("{} ", clue.html_text(puzzle))} </div>))
                        }</th>))}
                        </tr>
                    </thead>
                    <tbody>
                    {
                        puzzle.rows.iter().map(|row| html!(<tr><th class="row">{
                            row.iter().map(|clue| html!(<span style=(clue.html_color(puzzle))>{text!("{} ", clue.html_text(puzzle))} </span>))
                        }</th>
                        {
                            puzzle.cols.iter().map(|_| html!(<td></td>))
                        }
                        </tr>))
                    }
                    </tbody>
                </table>
            </body>
        </html>
    );

    html.to_string()
}

pub fn as_webpbn(document: &Document) -> String {
    use indoc::indoc;

    let mut document_with_puzzle = document.clone();
    let puzzle = document_with_puzzle.puzzle().assume_nono();

    let mut res = String::new();
    // If you add <!DOCTYPE pbn SYSTEM "https://webpbn.com/pbn-0.3.dtd">, `pbnsolve` emits a warning.
    res.push_str(indoc! {r#"
        <?xml version="1.0"?>
        <puzzleset>
        <puzzle type="grid" defaultcolor="white">
        <source>number-loom</source>
        "#});
    if let Some(title) = &document.title {
        res.push_str(&format!("<title>{}</title>\n", title));
    }
    if let Some(description) = &document.description {
        res.push_str(&format!("<description>{}</description>\n", description));
    }
    if let Some(author) = &document.author {
        res.push_str(&format!("<author>{}</author>\n", author));
    }
    for color in puzzle.palette.values() {
        let (r, g, b) = color.rgb;
        res.push_str(&format!(
            r#"<color name="{}" char="{}">{:02X}{:02X}{:02X}</color>"#,
            color.name, color.ch, r, g, b
        ));
        res.push('\n');
    }

    res.push_str(r#"<clues type="columns">"#);
    for column in &puzzle.cols {
        res.push_str("<line>");
        for clue in column {
            res.push_str(&format!(
                r#"<count color="{}">{}</count>"#,
                puzzle.palette[&clue.color].name, clue.count
            ));
        }
        res.push_str("</line>\n");
    }
    res.push_str(r#"</clues>"#);
    res.push('\n');

    res.push_str(r#"<clues type="rows">"#);
    for row in &puzzle.rows {
        res.push_str("<line>");
        for clue in row {
            res.push_str(&format!(
                r#"<count color="{}">{}</count>"#,
                puzzle.palette[&clue.color].name, clue.count
            ));
        }
        res.push_str("</line>\n");
    }
    res.push_str(r#"</clues>"#);
    res.push('\n');

    res.push_str(r#"</puzzle></puzzleset>"#);
    res.push('\n');

    res
}

pub fn olsak_ch(c: char, orig_to_sanitized: &mut HashMap<char, char>) -> char {
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

pub fn as_image_bytes<P>(solution: &Solution, path_or_filename: P) -> anyhow::Result<Vec<u8>>
where
    P: AsRef<Path>,
{
    let mut image = RgbImage::new(
        solution.grid.len() as u32,
        solution.grid.first().unwrap().len() as u32,
    );

    for (x, col) in solution.grid.iter().enumerate() {
        for (y, color) in col.iter().enumerate() {
            let color_info = &solution.palette[color];
            let (r, g, b) = color_info.rgb;
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

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, iter::FromIterator};

    use anyhow::bail;

    use crate::{
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

        let serialized = crate::export::as_olsak_triano(&p);

        println!("{}", serialized);

        let roundtripped = olsak_to_puzzle(&serialized).unwrap();

        println!("{:?}", roundtripped);

        puzzles_eq(&p, &roundtripped.assume_triano()).unwrap();
    }
}
