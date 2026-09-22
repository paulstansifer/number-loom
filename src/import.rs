use anyhow::{Context, bail};
use std::{collections::HashMap, io::Cursor, io::Read, path::PathBuf};

use typed_index_collections::TiSlice;

use crate::{
    formats::{
        char_grid::char_grid_to_solution, image::image_to_solution, olsak::olsak_to_puzzle,
        webpbn::webpbn_to_document, woven::from_woven,
    },
    geometry::{CellIdx, GridKind, LanePos, Square, Tri},
    puzzle::{
        self, BACKGROUND, Color, ColorInfo, Corner, Document, DynSolution, Nono, NonogramFormat,
        Puzzle, Solution, Triano,
    },
};

pub fn load_path(path: &PathBuf, format: Option<NonogramFormat>) -> anyhow::Result<Document> {
    let mut bytes = vec![];
    if path == &PathBuf::from("-") {
        std::io::stdin().read_to_end(&mut bytes)?;
    } else {
        bytes = std::fs::read(path)?;
    }

    load(
        path.to_str().context("path is not valid UTF-8")?,
        bytes,
        format,
    )
}

pub fn load(
    filename: &str,
    bytes: Vec<u8>,
    format: Option<NonogramFormat>,
) -> anyhow::Result<Document> {
    let input_format = puzzle::infer_format(filename, format);

    Ok(match input_format {
        NonogramFormat::Html => {
            bail!("HTML input is not supported.")
        }
        NonogramFormat::Image => {
            let img = image::load_from_memory(&bytes).context("could not decode image")?;
            let solution = image_to_solution(&img);
            Document::from_solution(DynSolution::Square(solution), filename.to_string())
        }
        NonogramFormat::Webpbn => {
            let webpbn_string = String::from_utf8(bytes).context("file is not valid UTF-8 text")?;
            let mut doc = webpbn_to_document(&webpbn_string)?;
            doc.file = filename.to_string();
            doc
        }
        NonogramFormat::CharGrid => {
            let grid_string = String::from_utf8(bytes).context("file is not valid UTF-8 text")?;
            let solution = char_grid_to_solution(&grid_string)?;
            Document::from_solution(solution, filename.to_string())
        }
        NonogramFormat::Woven => {
            let woven_string = String::from_utf8(bytes).context("file is not valid UTF-8 text")?;
            from_woven(&woven_string, filename.to_string())?
        }
        NonogramFormat::Olsak => {
            let olsak_string = String::from_utf8(bytes).context("file is not valid UTF-8 text")?;
            let puzzle = olsak_to_puzzle(&olsak_string)?;
            Document::from_puzzle(puzzle, filename.to_string())
        }
    })
}

pub fn solution_to_triano_puzzle(solution: &Solution<Square>) -> Puzzle<Triano, Square> {
    let width = solution.x_size();
    let height = solution.y_size();

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
            let color = solution[(x, y)];
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
            let color = solution[(x, y)];
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

    Puzzle::square(solution.palette.clone(), rows, cols)
}

/// Read off nonogram clues for one lane: maximal runs of a single non-background color.
fn clues_along_lane<K: GridKind>(
    solution: &Solution<K>,
    cells: &TiSlice<LanePos, CellIdx>,
) -> Vec<Nono> {
    let mut clues = Vec::<Nono>::new();

    let mut prev_color: Option<Color> = None;
    let mut run = 1;
    // One extra step past the end, so the final run gets flushed.
    for i in 0..cells.len() + 1 {
        let color = cells.get(LanePos::from(i)).map(|c| solution.cells[*c]);
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
    clues
}

/// Derive a puzzle's clues from a finished picture, for any geometry.
pub fn solution_to_nono_puzzle<K: GridKind>(solution: &Solution<K>) -> Puzzle<Nono, K> {
    let lanes = solution.geometry.lane_map();
    let lines = lanes
        .lanes()
        .keys()
        .map(|lane| clues_along_lane(solution, &lanes.lane(lane).cells))
        .collect();

    Puzzle {
        palette: solution.palette.clone(),
        geometry: solution.geometry.clone(),
        lines,
    }
}

pub fn solution_to_puzzle(solution: &Solution<Square>) -> Puzzle<Nono, Square> {
    solution_to_nono_puzzle(solution)
}

pub fn solution_to_tri_puzzle(solution: &Solution<Tri>) -> Puzzle<Nono, Tri> {
    solution_to_nono_puzzle(solution)
}

pub fn bw_palette() -> HashMap<Color, ColorInfo> {
    let mut palette = HashMap::new();
    palette.insert(BACKGROUND, ColorInfo::default_bg());
    palette.insert(Color(1), ColorInfo::default_fg(Color(1)));
    palette
}

/// The puzzle library, published by `.github/workflows/puzzle_archive.yml`:
const LIBRARY_URL: &str = "https://paulstansifer.github.io/number-loom/puzzles.zip";

/// Skip any stray files with extensions other than these:
const LIBRARY_EXTENSIONS: &[&str] = &["xml", "pbn", "txt", "g", "woven", "png", "gif", "bmp"];

pub async fn load_library() -> anyhow::Result<Vec<Document>> {
    load_zip_from_url(LIBRARY_URL).await
}

/// Fetches puzzles. Failing to reach or read the archive is an error; a puzzle inside it
/// that won't load is not (see `documents_from_zip`).
pub async fn load_zip_from_url(url: &str) -> anyhow::Result<Vec<Document>> {
    let response = reqwest::get(url)
        .await
        .with_context(|| format!("couldn't reach {url}"))?
        // A 404 is a perfectly good response as far as `reqwest` is concerned, and without this
        // we'd hand GitHub's 404 page to the zip reader and report "invalid Zip archive".
        .error_for_status()
        .with_context(|| format!("couldn't fetch {url}"))?;

    documents_from_zip(&response.bytes().await?)
}

/// Loads every puzzle in a zip archive, in filename order.
///
/// Directories, files we have no reader for, and files that fail to parse are all skipped.
/// Only an unreadable archive is an error.
pub fn documents_from_zip(zip_bytes: &[u8]) -> anyhow::Result<Vec<Document>> {
    let mut archive =
        zip::ZipArchive::new(Cursor::new(zip_bytes)).context("not a valid zip archive")?;

    let mut documents = vec![];

    for i in 0..archive.len() {
        let mut entry = match archive.by_index(i) {
            Ok(entry) => entry,
            Err(e) => {
                eprintln!("Skipping unreadable library entry: {e}");
                continue;
            }
        };

        if entry.is_dir() {
            continue;
        }

        let path = entry.name().to_string();
        let filename = path.rsplit('/').next().unwrap_or(&path).to_string();

        let readable = filename.rsplit_once('.').is_some_and(|(_, ext)| {
            LIBRARY_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str())
        });
        if !readable {
            continue;
        }

        let mut bytes = vec![];
        if let Err(e) = entry.read_to_end(&mut bytes) {
            eprintln!("Skipping library puzzle {path}: {e}");
            continue;
        }

        match load(&filename, bytes, None) {
            Ok(doc) => documents.push(doc),
            Err(e) => eprintln!("Skipping library puzzle {path}: {e:#}"),
        }
    }

    // Put in a canonical order:
    documents.sort_by(|a, b| a.file.cmp(&b.file));

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

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a zip in memory. `Stored` keeps this from depending on any of `zip`'s compression
    /// features; a name ending in `/` becomes a directory entry, as `zip -r` writes for a folder.
    fn zip_containing(entries: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::Write;

        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));

        for (name, bytes) in entries {
            if let Some(dir) = name.strip_suffix('/') {
                writer.add_directory(dir, options).unwrap();
            } else {
                writer.start_file(*name, options).unwrap();
                writer.write_all(bytes).unwrap();
            }
        }

        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn a_library_archive_yields_its_puzzles_in_order_without_their_directory_prefix() {
        // Stored out of alphabetical order on purpose: `ZipArchive` hands entries back in archive
        // order, but the gallery should look the same every time the archive is rebuilt. The
        // `puzzles/` prefix is what `zip -r puzzles.zip puzzles/` writes, and it must not survive
        // into `Document.file`, which the save dialog offers as a filename.
        let zip = zip_containing(&[
            ("puzzles/", b""),
            ("puzzles/zebra.txt", b"XX.\n.XX\n"),
            ("puzzles/apple.txt", b".X.\nXXX\n"),
        ]);

        let docs = documents_from_zip(&zip).unwrap();

        assert_eq!(
            docs.iter().map(|d| d.file.as_str()).collect::<Vec<_>>(),
            vec!["apple.txt", "zebra.txt"]
        );
    }

    #[test]
    fn an_unreadable_library_entry_is_skipped_instead_of_sinking_the_whole_library() {
        // `broken.xml` is a hard parse error. `README.md` is the subtler one: without the
        // extension check it would fall through `infer_format` to `CharGrid` and "succeed",
        // putting a puzzle made of prose in the gallery.
        let zip = zip_containing(&[
            ("good.txt", b"XX.\n.XX\n"),
            ("broken.xml", b"this is not xml at all"),
            ("README.md", b"Puzzles live here.\n"),
        ]);

        let docs = documents_from_zip(&zip).unwrap();

        assert_eq!(
            docs.iter().map(|d| d.file.as_str()).collect::<Vec<_>>(),
            vec!["good.txt"]
        );
    }

    #[test]
    fn something_that_is_not_a_zip_file_at_all_is_an_error() {
        // What a 404 page would look like coming back from the wrong URL: it has to be an error,
        // not an empty library that looks like the archive simply has no puzzles in it.
        assert!(documents_from_zip(b"<html>404: Not Found</html>").is_err());
    }
}
