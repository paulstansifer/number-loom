use std::{
    collections::HashMap,
    io::Cursor,
    io::Read,
    path::PathBuf,
};

use crate::{
    formats,
    puzzle::{
        self, BACKGROUND, Color, ColorInfo, Corner, Document, Nono,
        NonogramFormat, Puzzle, Solution, Triano,
    },
};

pub fn load_path(path: &PathBuf, format: Option<NonogramFormat>) -> Document {
    let mut bytes = vec![];
    if path == &PathBuf::from("-") {
        std::io::stdin().read_to_end(&mut bytes).unwrap();
    } else {
        bytes = std::fs::read(path).unwrap();
    }

    load(&path.to_str().unwrap(), bytes, format)
}

pub fn load(filename: &str, bytes: Vec<u8>, format: Option<NonogramFormat>) -> Document {
    let input_format = puzzle::infer_format(&filename, format);

    match input_format {
        NonogramFormat::Html => {
            panic!("HTML input is not supported.")
        }
        NonogramFormat::Image => {
            let img = image::load_from_memory(&bytes).unwrap();
            let solution = formats::image::image_to_solution(&img);
            Document::from_solution(solution, filename.to_string())
        }
        NonogramFormat::Webpbn => {
            let webpbn_string = String::from_utf8(bytes).unwrap();
            let mut doc = formats::webpbn::webpbn_to_document(&webpbn_string);
            doc.file = filename.to_string();
            doc
        }
        NonogramFormat::CharGrid => {
            let grid_string = String::from_utf8(bytes).unwrap();
            let solution = formats::char_grid::char_grid_to_solution(&grid_string);
            Document::from_solution(solution, filename.to_string())
        }
        NonogramFormat::Olsak => {
            let olsak_string = String::from_utf8(bytes).unwrap();
            let puzzle = formats::olsak::olsak_to_puzzle(&olsak_string).unwrap();
            Document::from_puzzle(puzzle, filename.to_string())
        }
    }
}

pub fn quality_check(solution: &Solution) {
    let width = solution.grid.len();
    let height = solution.grid.first().unwrap().len();

    let bg_squares_found: usize = solution
        .grid
        .iter()
        .map(|col| {
            col.iter()
                .map(|c| if *c == BACKGROUND { 1 } else { 0 })
                .sum::<usize>()
        })
        .sum();

    if bg_squares_found < (width + height) {
        eprintln!(
            "number-loom: warning: {} is a very small number of background squares",
            bg_squares_found
        );
    }

    if (width * height - bg_squares_found) < (width + height) {
        eprintln!(
            "number-loom: warning: {} is a very small number of foreground squares",
            width * height - bg_squares_found
        );
    }

    let num_colors = solution.palette.len();
    if num_colors > 30 {
        panic!(
            "{} colors detected. Nonograms with more than 30 colors are not supported.",
            num_colors
        );
    } else if num_colors > 10 {
        eprintln!(
            "number-loom: {} colors detected. That's probably too many.",
            num_colors
        )
    }

    // Find similar colors
    for (color_key, color) in &solution.palette {
        for (color_key2, color2) in &solution.palette {
            if color_key == color_key2 {
                continue;
            }
            if color.corner != color2.corner && color.rgb == color2.rgb {
                continue; // Corners may be the same color.
            }
            let (r, g, b) = color.rgb;
            let (r2, g2, b2) = color2.rgb;
            if (r2 as i16 - r as i16).abs()
                + (g2 as i16 - g as i16).abs()
                + (b2 as i16 - b as i16).abs()
                < 30
            {
                eprintln!(
                    "number-loom: warning: very similar colors found: {:?} and {:?}",
                    color.rgb, color2.rgb
                );
            }
        }
    }
}

pub fn solution_to_triano_puzzle(solution: &Solution) -> Puzzle<Triano> {
    let width = solution.grid.len();
    let height = solution.grid.first().unwrap().len();

    let mut rows: Vec<Vec<Triano>> = Vec::new();
    let mut cols: Vec<Vec<Triano>> = Vec::new();

    let blank_clue = Triano {
        front_cap: None,
        body_color: BACKGROUND,
        body_len: 0,
        back_cap: None,
    };

    // Generate row clues
    for y in 0..height {
        let mut clues = Vec::<Triano>::new();
        let mut cur_clue = blank_clue;

        for x in 0..width {
            let color = solution.grid[x][y];
            let color_info = &solution.palette[&color];

            // For example `!left` means ◢ or ◥:
            if color_info.corner.is_some_and(|c| !c.left) {
                // Only a blank clue can accept a front cap:
                if cur_clue != blank_clue {
                    clues.push(cur_clue);
                    cur_clue = blank_clue
                }
                cur_clue.front_cap = Some(color);
            } else if color_info.corner.is_some_and(|c| c.left) {
                // The back cap is always none...
                cur_clue.back_cap = Some(color);
                // ...because we finish right after setting it
                clues.push(cur_clue);
                cur_clue = blank_clue;
            } else if color == BACKGROUND {
                if cur_clue != blank_clue {
                    clues.push(cur_clue);
                    cur_clue = blank_clue;
                }
            } else {
                // Since the back cap is always none, the only obstacle to continuing is if the
                // body color is wrong.
                if cur_clue.body_color != BACKGROUND && cur_clue.body_color != color {
                    clues.push(cur_clue);
                    cur_clue = blank_clue;
                }
                cur_clue.body_color = color;
                cur_clue.body_len += 1;
            }
        }
        if cur_clue != blank_clue {
            clues.push(cur_clue);
        }

        rows.push(clues);
    }

    // Generate column clues
    for x in 0..width {
        let mut clues = Vec::<Triano>::new();
        let mut cur_clue = blank_clue;

        for y in 0..height {
            let color = solution.grid[x][y];
            let color_info = &solution.palette[&color];

            if color_info.corner.is_some_and(|c| !c.upper) {
                // Only a blank clue can accept a front cap:
                if cur_clue != blank_clue {
                    clues.push(cur_clue);
                    cur_clue = blank_clue
                }
                cur_clue.front_cap = Some(color);
            } else if color_info.corner.is_some_and(|c| c.upper) {
                // The back cap is always none...
                cur_clue.back_cap = Some(color);
                // ...because we finish right after setting it
                clues.push(cur_clue);
                cur_clue = blank_clue;
            } else if color == BACKGROUND {
                if cur_clue != blank_clue {
                    clues.push(cur_clue);
                    cur_clue = blank_clue;
                }
            } else {
                // Since the back cap is always none, the only obstacle to continuing is if the
                // body color is wrong.
                if cur_clue.body_color != BACKGROUND && cur_clue.body_color != color {
                    clues.push(cur_clue);
                    cur_clue = blank_clue;
                }
                cur_clue.body_color = color;
                cur_clue.body_len += 1;
            }
        }
        if cur_clue != blank_clue {
            clues.push(cur_clue);
        }

        cols.push(clues);
    }

    Puzzle {
        palette: solution.palette.clone(),
        rows,
        cols,
    }
}

pub fn solution_to_puzzle(solution: &Solution) -> Puzzle<Nono> {
    let width = solution.grid.len();
    let height = solution.grid.first().unwrap().len();

    let mut rows: Vec<Vec<Nono>> = Vec::new();
    let mut cols: Vec<Vec<Nono>> = Vec::new();

    // Generate row clues
    for y in 0..height {
        let mut clues = Vec::<Nono>::new();

        let mut prev_color: Option<Color> = None;
        let mut run = 1;
        for x in 0..width + 1 {
            let color = if x < width {
                Some(solution.grid[x][y])
            } else {
                None
            };
            if prev_color == color {
                run += 1;
                continue;
            }
            match prev_color {
                None => {}
                Some(color) if color == puzzle::BACKGROUND => {}
                Some(color) => clues.push(Nono { color, count: run }),
            }
            prev_color = color;
            run = 1;
        }
        rows.push(clues);
    }

    // Generate column clues
    for x in 0..width {
        let mut clues = Vec::<Nono>::new();

        let mut prev_color = None;
        let mut run = 1;
        for y in 0..height + 1 {
            let color = if y < height {
                Some(solution.grid[x][y])
            } else {
                None
            };
            if prev_color == color {
                run += 1;
                continue;
            }
            match prev_color {
                None => {}
                Some(color) if color == BACKGROUND => {}
                Some(color) => clues.push(Nono { color, count: run }),
            }
            prev_color = color;
            run = 1;
        }
        cols.push(clues);
    }

    Puzzle {
        palette: solution.palette.clone(),
        rows,
        cols,
    }
}

pub fn bw_palette() -> HashMap<Color, ColorInfo> {
    let mut palette = HashMap::new();
    palette.insert(BACKGROUND, ColorInfo::default_bg());
    palette.insert(Color(1), ColorInfo::default_fg(Color(1)));
    palette
}

// It's impossible to get released assests from GitHub for CORS reasons (!?), so
// we grab the raw files:
pub async fn puzzles_from_github() -> anyhow::Result<Vec<Document>> {
    let client = reqwest::Client::new();

    let puzzles_url =
        "https://api.github.com/repos/paulstansifer/number-loom/contents/puzzles?ref=main";

    let contents = client
        .get(puzzles_url)
        .header("User-Agent", "number-loom")
        .send()
        .await?
        .bytes()
        .await?;

    let files: Vec<serde_json::Value> = serde_json::from_slice(&contents)?;

    let mut res: Vec<Document> = vec![];

    for file in files {
        if file["type"] == "file" {
            let name = file["name"].as_str().unwrap();
            let download_url = file["download_url"].as_str().unwrap();

            let content = client.get(download_url).send().await?.bytes().await?;

            res.push(load(name, content.to_vec(), None));
        }
    }

    Ok(res)
}

pub async fn load_zip_from_url(url: &str) -> anyhow::Result<Vec<Document>> {
    let response = reqwest::get(url).await?;
    let zip_bytes = response.bytes().await?;
    let zip_cursor = Cursor::new(zip_bytes);

    let mut archive = zip::ZipArchive::new(zip_cursor)?;
    let mut documents = vec![];

    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        let filename = file.name().to_string();

        if file.is_dir() {
            continue;
        }

        let mut bytes = vec![];
        file.read_to_end(&mut bytes)?;
        documents.push(load(&filename, bytes, None));
    }

    Ok(documents)
}

pub fn triano_palette() -> HashMap<Color, ColorInfo> {
    let mut palette = HashMap::new();
    palette.insert(BACKGROUND, ColorInfo::default_bg());
    palette.insert(Color(1), ColorInfo::default_fg(Color(1)));

    palette.insert(
        Color(3),
        ColorInfo {
            ch: '◤',
            name: r#"black/white"#.to_string(),
            rgb: (0, 0, 0),
            color: Color(3),
            corner: Some(Corner {
                upper: true,
                left: true,
            }),
        },
    );
    palette.insert(
        Color(4),
        ColorInfo {
            ch: '◥',
            name: r#"white\black"#.to_string(),
            rgb: (0, 0, 0),
            color: Color(4),
            corner: Some(Corner {
                upper: true,
                left: false,
            }),
        },
    );
    palette.insert(
        Color(5),
        ColorInfo {
            ch: '◣',
            name: r#"black\white"#.to_string(),
            rgb: (0, 0, 0),
            color: Color(5),
            corner: Some(Corner {
                upper: false,
                left: true,
            }),
        },
    );
    palette.insert(
        Color(6),
        ColorInfo {
            ch: '◢',
            name: r#"white/black"#.to_string(),
            rgb: (0, 0, 0),
            color: Color(6),
            corner: Some(Corner {
                upper: false,
                left: false,
            }),
        },
    );

    palette
}
