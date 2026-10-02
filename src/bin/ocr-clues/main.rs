//! Reads the clues out of a picture of a nonogram (a screenshot, a scan, a photo).
//!
//! ```text
//! cargo run --release --features ocr --bin ocr-clues -- puzzle.webp out.xml --debug-image debug.png
//! ```
//!
//! OCR (the `ocrs` crate) finds the digits; `clue_layout` works out which of them are
//! clues and how they line up. The OCR models aren't bundled; see `DEVELOPING.md`.

use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use clap::Parser;
use clue_layout::{ClueLayout, Expected, Glyph, Role, Score, arrange};
use number_loom::puzzle::{
    BACKGROUND, Color, Document, DynPuzzle, DynSolution, Nono, NonogramFormat, Puzzle,
    PuzzleDynOps, UNSOLVED,
};
use number_loom::solve::grid_solve::Report;
use number_loom::{export, import};
use ocrs::{ImageSource, OcrEngine, OcrEngineParams, OcrInput, TextItem};
use rten::Model;
use rten_imageproc::{RectF, RetrievalMode, RotatedRect, find_contours};
use rten_tensor::NdTensorView;
use rten_tensor::prelude::*;

mod cells;
mod clue_layout;
mod grid;
mod reread;
mod templates;
mod warp;

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
enum Dewarp {
    Auto,
    Always,
    Never,
}

#[derive(clap::Parser, Debug)]
#[command(about = "Read nonogram clues out of a picture")]
struct Args {
    /// The picture of the puzzle
    #[arg(required_unless_present = "score")]
    image: Option<PathBuf>,

    /// Where to write the puzzle; "-" (the default) for stdout
    output: Option<PathBuf>,

    /// Format to write the puzzle in (by default, guessed from the output's file name, or olsak
    /// for stdout)
    #[arg(short, long, value_enum)]
    output_format: Option<NonogramFormat>,

    /// The number of columns, for when there are empty columns at the right, which leave no clues
    /// to find
    #[arg(long)]
    width: Option<usize>,

    /// The number of rows, for when there are empty rows at the bottom
    #[arg(long)]
    height: Option<usize>,

    /// Read every picture in this directory, and compare each to the clues in the `.clues` file
    /// beside it. Where there isn't one yet, write what was read, for a human to correct.
    #[arg(long, conflicts_with_all = ["image", "output"])]
    score: Option<PathBuf>,

    /// With --score, write each picture's debug image (see --debug-image) into this directory
    #[arg(long, requires = "score")]
    debug_dir: Option<PathBuf>,

    /// Skip the second pass, which re-reads each lane where the first pass says it is
    #[arg(long)]
    no_reread: bool,

    /// In the second pass, compare each digit to the others to catch misreadings (experimental:
    /// it hasn't helped yet, on the pictures we have)
    #[arg(long)]
    compare_digits: bool,

    /// Straighten out the grid's lines first, for photos of paper that isn't flat. ("auto" does
    /// it when the picture as it is doesn't make sense, and straightening it helps.)
    #[arg(long, value_enum, default_value = "auto")]
    dewarp: Dewarp,

    /// When straightening, draw the lines it traced on a copy of the original picture
    #[arg(long)]
    debug_lines: Option<PathBuf>,

    /// Print the digits OCR found (digit, center x, center y, width, height), and stop
    #[arg(long)]
    dump_glyphs: bool,

    /// The text-detection model. Defaults to `~/.cache/ocrs/text-detection.{rten,onnx}`
    #[arg(long)]
    detection_model: Option<PathBuf>,

    /// The text-recognition model. Defaults to `~/.cache/ocrs/text-recognition.{rten,onnx}`
    #[arg(long)]
    recognition_model: Option<PathBuf>,

    /// Write a copy of the picture marked up with what was found where
    #[arg(long)]
    debug_image: Option<PathBuf>,

    /// How sure the OCR model must be that a pixel is part of some text
    #[arg(long, default_value_t = 0.15)]
    threshold: f32,

    /// The smallest word OCR will consider, in square pixels
    #[arg(long, default_value_t = 20.0)]
    min_area: f32,

    /// Enlarge the picture by this factor before OCR; helps when the digits are tiny
    #[arg(long, default_value_t = 1.0)]
    scale: f32,
}

const MODEL_URL: &str = "https://ocrs-models.s3-accelerate.amazonaws.com";

/// Find a model in `~/.cache/ocrs/`, where `ocrs-cli` keeps them.
fn default_model(name: &str) -> anyhow::Result<PathBuf> {
    let home = std::env::var_os("HOME").context("no $HOME to find models in")?;
    let dir = Path::new(&home).join(".cache/ocrs");
    for ext in ["rten", "onnx"] {
        let path = dir.join(format!("{name}.{ext}"));
        if path.exists() {
            return Ok(path);
        }
    }
    bail!(
        "no {name} model in {dir:?}; download it with\n  \
         curl {MODEL_URL}/{name}.onnx -o {}",
        dir.join(format!("{name}.onnx")).display()
    )
}

fn load_model(path: Option<PathBuf>, name: &str) -> anyhow::Result<Model> {
    let path = match path {
        Some(path) => path,
        None => default_model(name)?,
    };
    Model::load_file(&path).with_context(|| format!("loading {path:?}"))
}

/// OCR misreads a lone digit as a similar-looking letter now and then.
fn as_digit(c: char) -> Option<u8> {
    match c {
        '0'..='9' => Some(c as u8 - b'0'),
        'l' | 'I' | '|' | 'i' | '!' => Some(1),
        'O' | 'o' => Some(0),
        _ => None,
    }
}

/// Text OCR read that isn't made of digits (kept only to draw on the debug image).
struct Rejected {
    text: String,
    rect: (f32, f32, f32, f32),
}

/// A blob of text found by detection: the box around the pixels the model marked.
#[derive(Clone, Copy, Debug)]
struct Blob {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

impl Blob {
    fn height(&self) -> f32 {
        (self.bottom - self.top) as f32 + 2.0 * EXPAND
    }
    fn width(&self) -> f32 {
        (self.right - self.left) as f32 + 2.0 * EXPAND
    }

    /// The area to hand to recognition.
    fn rect(&self) -> RotatedRect {
        // The model marks a little less than the whole word, so pad it (as `ocrs` does). And a
        // lone "1" is so thin that recognition, given only what was marked, reads it as junk.
        let (top, bottom) = (self.top as f32 - EXPAND, self.bottom as f32 + EXPAND);
        let half_width = (self.width() / 2.0).max(self.height() * 0.35);
        let center = (self.left + self.right) as f32 / 2.0;
        RotatedRect::from_rect(RectF::from_tlbr(
            top,
            center - half_width,
            bottom,
            center + half_width,
        ))
    }
}

const EXPAND: f32 = 3.0;

/// Like `OcrEngine::detect_words`, but with an adjustable threshold and minimum size. Its
/// defaults are tuned for prose, and miss a lone "1", or a clue grayed out to show it's done.
fn find_blobs(mask: NdTensorView<bool, 2>, min_area: f32) -> Vec<Blob> {
    let mut blobs = vec![];
    for poly in find_contours(mask, RetrievalMode::External).iter() {
        let (mut left, mut top, mut right, mut bottom) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        for p in poly.iter() {
            (left, top) = (left.min(p.x), top.min(p.y));
            (right, bottom) = (right.max(p.x), bottom.max(p.y));
        }
        let blob = Blob {
            left,
            top,
            right,
            bottom,
        };
        if blob.width() * blob.height() >= min_area {
            blobs.push(blob);
        }
    }
    blobs
}

/// Column clues can be stacked so tightly that detection sees a tower of them as one blob.
/// Cut it up where the model is least sure there's text, into pieces about `height` tall.
fn split_tower(blob: &Blob, probs: NdTensorView<f32, 2>, height: f32) -> Vec<Blob> {
    let glyph = height - 2.0 * EXPAND;
    let tall = (blob.bottom - blob.top) as f32;
    // Stacked digits have a little space between them.
    let pieces = ((tall + 0.3 * glyph) / (1.3 * glyph)).round().max(2.0) as i32;
    let profile = |y: i32| -> f32 {
        (blob.left..=blob.right)
            .map(|x| probs[[y as usize, x as usize]])
            .sum()
    };
    let step = tall / pieces as f32;
    let mut cuts = vec![blob.top];
    for i in 1..pieces {
        let ideal = blob.top as f32 + i as f32 * step;
        let window = (ideal - 0.35 * step) as i32..=(ideal + 0.35 * step) as i32;
        let cut = window
            .min_by(|&a, &b| profile(a).total_cmp(&profile(b)))
            .unwrap();
        cuts.push(cut);
    }
    cuts.push(blob.bottom);
    cuts.windows(2)
        .map(|w| Blob {
            top: w[0] + 1,
            bottom: w[1] - 1,
            ..*blob
        })
        .filter(|b| b.bottom > b.top)
        .collect()
}

/// Recognize each blob as a line of its own: `find_text_lines` would string together
/// neighboring clues, and even neighboring columns, which is exactly the decision we want to make
/// ourselves. Returns the digits, tagged with the blob they're from, and the text that wasn't
/// made of digits.
fn recognize(
    engine: &OcrEngine,
    input: &OcrInput,
    blobs: &[Blob],
) -> anyhow::Result<(Vec<(usize, Glyph)>, Vec<Rejected>)> {
    let lines: Vec<_> = blobs.iter().map(|b| vec![b.rect()]).collect();
    let mut glyphs = vec![];
    let mut rejected = vec![];
    for (blob, line) in engine
        .recognize_text(input, &lines)?
        .into_iter()
        .enumerate()
    {
        for word in line.iter().flat_map(|l| l.words()) {
            let Some(digits) = word
                .chars()
                .iter()
                .map(|c| as_digit(c.char))
                .collect::<Option<Vec<u8>>>()
            else {
                let r = word.bounding_rect();
                rejected.push(Rejected {
                    text: word.to_string(),
                    rect: (
                        r.left() as f32,
                        r.top() as f32,
                        r.right() as f32,
                        r.bottom() as f32,
                    ),
                });
                continue;
            };
            for (c, digit) in word.chars().iter().zip(digits) {
                let r = c.rect;
                let glyph = Glyph {
                    digit,
                    x: (r.left() + r.right()) as f32 / 2.0,
                    y: (r.top() + r.bottom()) as f32 / 2.0,
                    width: r.width() as f32,
                    height: r.height() as f32,
                };
                glyphs.push((blob, glyph));
            }
        }
    }
    Ok((glyphs, rejected))
}

/// Every digit in the image. Words that aren't entirely digits are dropped: they're titles,
/// buttons, and status bars, not clues.
fn find_glyphs(
    engine: &OcrEngine,
    input: &OcrInput,
    threshold: f32,
    min_area: f32,
    flattened: bool,
) -> anyhow::Result<(Vec<Glyph>, Vec<Rejected>)> {
    let probs = engine.detect_text_pixels(input)?;
    // In a flattened picture, detection gives everything a fair chance of being text, so the
    // threshold has to clear that. (Not otherwise: in some pictures, faint clues are only just
    // above the background.)
    let threshold = if flattened {
        let mut sample: Vec<f32> = probs.iter().step_by(97).copied().collect();
        sample.sort_by(f32::total_cmp);
        threshold.max(sample.get(sample.len() / 2).copied().unwrap_or(0.0) + 0.05)
    } else {
        threshold
    };
    let mask = probs.map(|p| *p > threshold);
    let blobs = find_blobs(mask.view(), min_area);
    let (mut glyphs, mut rejected) = recognize(engine, input, &blobs)?;

    let median_height = |glyphs: &[(usize, Glyph)]| {
        let mut heights: Vec<f32> = glyphs.iter().map(|(_, g)| g.height).collect();
        heights.sort_by(f32::total_cmp);
        heights.get(heights.len() / 2).copied()
    };
    let Some(height) = median_height(&glyphs) else {
        return Ok((vec![], rejected));
    };

    // Narrow blobs that are too tall are towers of column clues: split them and try again.
    let is_tower = |b: &Blob| b.height() > 1.6 * height && b.width() < 3.0 * height;
    let towers: Vec<Blob> = blobs.iter().filter(|b| is_tower(b)).copied().collect();
    if !towers.is_empty() {
        glyphs.retain(|(blob, _)| !is_tower(&blobs[*blob]));
        let pieces: Vec<Blob> = towers
            .iter()
            .flat_map(|t| split_tower(t, probs.view(), height))
            .collect();
        let (more_glyphs, more_rejected) = recognize(engine, input, &pieces)?;
        glyphs.extend(more_glyphs);
        rejected.extend(more_rejected);
    }

    // A low detection threshold picks up faint clues, but also the texture of the grid, which
    // OCR sometimes reads as digits. Those come in the wrong sizes: specks, or big blobs.
    let mut result = vec![];
    for (_, g) in glyphs {
        if (0.6..=1.6).contains(&(g.height / height)) {
            result.push(g);
        } else {
            rejected.push(Rejected {
                text: format!("{}?", g.digit),
                rect: (
                    g.x - g.width / 2.0,
                    g.y - g.height / 2.0,
                    g.x + g.width / 2.0,
                    g.y + g.height / 2.0,
                ),
            });
        }
    }
    Ok((result, rejected))
}

/// Drawing on the debug image.
mod draw {
    use ab_glyph::{Font as _, FontRef, PxScale, ScaleFont as _};
    use image::{Rgb, RgbImage};
    use std::sync::LazyLock;

    static FONT: LazyLock<FontRef<'static>> = LazyLock::new(|| {
        FontRef::try_from_slice(epaint_default_fonts::HACK_REGULAR).expect("bundled font")
    });

    fn blend(img: &mut RgbImage, x: i64, y: i64, color: Rgb<u8>, alpha: f32) {
        if x < 0 || y < 0 || x >= img.width() as i64 || y >= img.height() as i64 {
            return;
        }
        let px = img.get_pixel_mut(x as u32, y as u32);
        for i in 0..3 {
            px[i] = (px[i] as f32 * (1.0 - alpha) + color[i] as f32 * alpha) as u8;
        }
    }

    pub fn rect(img: &mut RgbImage, (x0, y0, x1, y1): (f32, f32, f32, f32), color: Rgb<u8>) {
        let (x0, y0, x1, y1) = (x0 as i64, y0 as i64, x1 as i64, y1 as i64);
        for t in 0..2 {
            for x in x0..=x1 {
                blend(img, x, y0 - t, color, 1.0);
                blend(img, x, y1 + t, color, 1.0);
            }
            for y in y0..=y1 {
                blend(img, x0 - t, y, color, 1.0);
                blend(img, x1 + t, y, color, 1.0);
            }
        }
    }

    pub fn line(img: &mut RgbImage, (x0, y0): (f32, f32), (x1, y1): (f32, f32), color: Rgb<u8>) {
        let steps = (x1 - x0).abs().max((y1 - y0).abs()).ceil().max(1.0) as i64;
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            let (x, y) = (x0 + (x1 - x0) * t, y0 + (y1 - y0) * t);
            blend(img, x as i64, y as i64, color, 1.0);
            blend(img, x as i64 + 1, y as i64, color, 1.0);
        }
    }

    /// `text`, with its top-left corner at `(x, y)`.
    pub fn text(img: &mut RgbImage, (x, y): (f32, f32), size: f32, text: &str, color: Rgb<u8>) {
        let font = FONT.as_scaled(PxScale::from(size));
        let mut caret = x;
        for c in text.chars() {
            let mut glyph = font.scaled_glyph(c);
            let advance = font.h_advance(glyph.id);
            glyph.position = ab_glyph::point(caret, y + font.ascent());
            if let Some(outlined) = FONT.outline_glyph(glyph) {
                let bounds = outlined.px_bounds();
                outlined.draw(|gx, gy, coverage| {
                    blend(
                        img,
                        bounds.min.x as i64 + gx as i64,
                        bounds.min.y as i64 + gy as i64,
                        color,
                        coverage,
                    )
                });
            }
            caret += advance;
        }
    }
}

/// The picture at `path`, scaled as requested.
fn open_image(path: &Path, scale: f32) -> anyhow::Result<image::RgbImage> {
    let mut image = image::open(path)
        .with_context(|| format!("reading {path:?}"))?
        .into_rgb8();
    if scale != 1.0 {
        let (w, h) = image.dimensions();
        image = image::imageops::resize(
            &image,
            (w as f32 * scale) as u32,
            (h as f32 * scale) as u32,
            image::imageops::FilterType::CatmullRom,
        );
    }
    Ok(image)
}

fn show(clues: &[Vec<u16>]) -> String {
    clues
        .iter()
        .map(|c| {
            c.iter()
                .map(|&n| clue_layout::clue_text(n))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Everything found in one picture.
struct Reading {
    image: image::RgbImage,
    glyphs: Vec<Glyph>,
    rejected: Vec<Rejected>,
    /// With the clues from the second pass, if there was one.
    layout: anyhow::Result<ClueLayout>,
    /// The grid, as found from its lines.
    grid: Option<grid::Grid>,
    /// Whether the picture was straightened out first (and `image` is the straightened one).
    flattened: bool,
    reread: Option<reread::Reread>,
}

/// The picture, straightened out (see `warp`), if its lines can be traced, and the size of its
/// cells.
fn straighten(
    image: &image::RgbImage,
    args: &Args,
) -> anyhow::Result<Option<(image::RgbImage, f32)>> {
    let (w, h) = (image.width() as f32, image.height() as f32);
    let Some((mesh, traced)) =
        warp::trace(image).and_then(|traced| Some((warp::Mesh::new(&traced, w, h)?, traced)))
    else {
        return Ok(None);
    };
    if let Some(lines_path) = &args.debug_lines {
        // As traced in red and blue, and as smoothed in green.
        let mut debug = image.clone();
        for (lines, color) in [
            (&traced.horizontal, image::Rgb([230, 0, 0])),
            (&traced.vertical, image::Rgb([0, 60, 255])),
        ] {
            for (_, curve) in lines {
                for w in curve.windows(2) {
                    draw::line(&mut debug, w[0], w[1], color);
                }
            }
        }
        for curve in mesh.lines(w, h) {
            for w in curve.windows(2) {
                draw::line(&mut debug, w[0], w[1], image::Rgb([0, 180, 0]));
            }
        }
        debug
            .save(lines_path)
            .with_context(|| format!("writing {lines_path:?}"))?;
    }
    let cell = traced.pitch.round().max(8.0);
    Ok(Some((warp::flatten(image, &mesh, cell), cell)))
}

/// How much sense a reading makes, from 0 (none) to 1 (it's consistent: the rows and columns
/// fill the same number of cells, every clue fits, and none is blotted).
fn sense(layout: &anyhow::Result<ClueLayout>) -> f32 {
    let Ok(layout) = layout else {
        return 0.0;
    };
    let (width, height) = (layout.cols.len(), layout.rows.len());
    let lanes = width + height;
    if lanes == 0 {
        return 0.0;
    }
    let clues = |lanes: &[Vec<u16>]| -> Vec<u16> {
        lanes
            .iter()
            .flatten()
            .copied()
            .filter(|&n| n != clue_layout::BLOTTED)
            .collect()
    };
    let (cols, rows) = (clues(&layout.cols), clues(&layout.rows));
    let total = |c: &[u16]| c.iter().map(|&n| n as f32).sum::<f32>();
    let (sc, sr) = (total(&cols), total(&rows));
    if sc + sr == 0.0 {
        return 0.0;
    }
    let agreement = 1.0 - (sc - sr).abs() / sc.max(sr);
    let fits = |lanes: &[Vec<u16>], len: usize| {
        lanes
            .iter()
            .filter(|lane| {
                lane.iter().map(|&n| n as usize).sum::<usize>() + lane.len().saturating_sub(1)
                    <= len
            })
            .count()
    };
    let fitting = (fits(&layout.cols, height) + fits(&layout.rows, width)) as f32 / lanes as f32;
    let numbers = layout.cols.iter().chain(&layout.rows).flatten().count();
    let read = (cols.len() + rows.len()) as f32 / numbers.max(1) as f32;
    agreement * fitting * read
}

fn read(engine: &OcrEngine, path: &Path, args: &Args) -> anyhow::Result<Reading> {
    let image = open_image(path, args.scale)?;
    match args.dewarp {
        Dewarp::Never => read_image(engine, image, None, args),
        Dewarp::Always => match straighten(&image, args)? {
            Some((flat, cell)) => read_image(engine, flat, Some(cell), args),
            None => {
                eprintln!("Warning: couldn't trace the grid's lines to straighten them");
                read_image(engine, image, None, args)
            }
        },
        // Straightening a picture that doesn't need it only loses detail, so try without first.
        Dewarp::Auto => {
            let plain = read_image(engine, image.clone(), None, args)?;
            let plain_sense = sense(&plain.layout);
            if plain_sense >= 1.0 {
                return Ok(plain);
            }
            let Some((flat, cell)) = straighten(&image, args)? else {
                return Ok(plain);
            };
            let flattened = read_image(engine, flat, Some(cell), args)?;
            // (A clear improvement, not just a different set of mistakes.)
            Ok(if sense(&flattened.layout) > plain_sense + 0.1 {
                flattened
            } else {
                plain
            })
        }
    }
}

/// `flattened_cell` is the size of a cell, if the picture was straightened (which makes them all
/// exactly that).
fn read_image(
    engine: &OcrEngine,
    image: image::RgbImage,
    flattened_cell: Option<f32>,
    args: &Args,
) -> anyhow::Result<Reading> {
    let flattened = flattened_cell.is_some();
    let source = ImageSource::from_bytes(image.as_raw(), image.dimensions())?;
    let input = engine.prepare_input(source)?;
    let (glyphs, rejected) = find_glyphs(engine, &input, args.threshold, args.min_area, flattened)?;
    let mut layout = arrange(&glyphs, args.width, args.height);
    // The size of a cell, if it's known: exactly, from straightening; or roughly, from the
    // clues (if their columns and rows agree).
    let cell = flattened_cell.or_else(|| {
        let found = layout.as_ref().ok()?;
        let (a, b) = (found.col_pitch, found.row_pitch);
        ((a - b).abs() < 0.1 * a.max(b)).then_some((a + b) / 2.0)
    });
    let grid = grid::find(&image, cell);
    let mut from_lines = false;
    // Where the grid's lines can be found, they're a surer guide to its shape than the clues.
    // The grid can also be found from its lines, but that's less reliable than the clues, if
    // the clues can be read (it can mistake boxes around clues for the edge of the grid). So it's
    // only for when the clues gave nothing sensible: no grid, or cells far from square.
    if let Some(grid) = &grid {
        let use_grid = grid.width() >= 5
            && grid.height() >= 5
            && match &layout {
                Err(_) => true,
                Ok(found) => {
                    let (a, b) = (found.col_pitch, found.row_pitch);
                    found.cols.is_empty()
                        || found.rows.is_empty()
                        || (a - b).abs() > 0.25 * a.max(b)
                }
            };
        if use_grid {
            let height = match &layout {
                Ok(found) => found.glyph_height,
                Err(_) => {
                    let mut heights: Vec<f32> = glyphs.iter().map(|g| g.height).collect();
                    heights.sort_by(f32::total_cmp);
                    heights
                        .get(heights.len() / 2)
                        .copied()
                        .unwrap_or(0.6 * grid.horizontal.pitch)
                }
            };
            let mut from_grid = grid.layout(height, glyphs.len());
            from_grid
                .warnings
                .push("the clues didn't make sense, so the grid was found from its lines".into());
            layout = Ok(from_grid);
            from_lines = true;
        }
    }
    // Where the grid's lines can be seen, they say exactly where each lane is: the clues only
    // say roughly. (Only if the two agree on the size of a cell.)
    if let (Some(grid), Ok(layout)) = (&grid, &mut layout) {
        let close = |a: f32, b: f32| (a - b).abs() < 0.1 * b;
        if close(layout.col_pitch, grid.vertical.pitch)
            && close(layout.row_pitch, grid.horizontal.pitch)
        {
            grid.snap(layout);
        }
    }
    let mut reread = None;
    let mut failed = None;
    if let Ok(layout) = &mut layout
        && !args.no_reread
    {
        match reread::reread(
            engine,
            &input,
            &image,
            layout,
            args.compare_digits,
            // A grid found from its lines likely has boxes for its clues, too.
            from_lines,
        ) {
            Ok(again) => {
                layout.cols = again.cols.clone();
                layout.rows = again.rows.clone();
                reread = Some(again);
            }
            // (A layout so broken there's nothing to re-read is no layout at all.)
            Err(e) => failed = Some(e),
        }
    }
    if let Some(e) = failed {
        layout = Err(e);
    }
    Ok(Reading {
        image,
        glyphs,
        rejected,
        layout,
        grid,
        flattened,
        reread,
    })
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let engine = OcrEngine::new(OcrEngineParams {
        detection_model: Some(load_model(args.detection_model.clone(), "text-detection")?),
        recognition_model: Some(load_model(
            args.recognition_model.clone(),
            "text-recognition",
        )?),
        ..Default::default()
    })?;

    if let Some(dir) = &args.score {
        return score(&engine, dir, &args);
    }
    let reading = read(
        &engine,
        args.image.as_ref().expect("clap requires it"),
        &args,
    )?;
    if args.dump_glyphs {
        for g in &reading.glyphs {
            println!("{} {} {} {} {}", g.digit, g.x, g.y, g.width, g.height);
        }
        return Ok(());
    }
    let save_debug_image = |states: Option<&[Vec<cells::State>]>| -> anyhow::Result<()> {
        if let Some(path) = &args.debug_image {
            debug_image(&reading, states)
                .save(path)
                .with_context(|| format!("writing {path:?}"))?;
        }
        Ok(())
    };
    let layout = match &reading.layout {
        Ok(layout) => layout,
        Err(e) => {
            save_debug_image(None)?;
            anyhow::bail!("{e:#}");
        }
    };
    if reading.flattened {
        eprintln!("(Straightened out the picture's lines first.)");
    }
    if let Some(reread) = &reading.reread {
        eprintln!(
            "(The second pass fell back on the first for {} lanes, couldn't read {} clues, and \
             corrected {} numbers by comparing digits.)",
            reread.fallbacks, reread.blots, reread.corrections
        );
    }
    eprintln!("{} columns: {}", layout.cols.len(), show(&layout.cols));
    eprintln!("{} rows: {}", layout.rows.len(), show(&layout.rows));
    for warning in &layout.warnings {
        eprintln!("Warning: {warning}");
    }

    let as_nonos = |clues: &[Vec<u16>]| -> Vec<Vec<Nono>> {
        clues
            .iter()
            .map(|c| {
                c.iter()
                    .map(|&count| Nono {
                        color: Color(1),
                        count,
                    })
                    .collect()
            })
            .collect()
    };
    let puzzle = Puzzle::square(
        import::bw_palette(),
        as_nonos(&layout.rows),
        as_nonos(&layout.cols),
    );
    // The state of the grid: what's been filled in and crossed out so far.
    let states = cells::read(&reading.image, layout);
    eprintln!("The grid (# filled in, x crossed out, . undecided):");
    for row in &states {
        let line: String = row
            .iter()
            .map(|state| match state {
                cells::State::Filled => '#',
                cells::State::Crossed => 'x',
                cells::State::Undecided => '.',
            })
            .collect();
        eprintln!("    {line}");
    }

    let blotted = layout
        .cols
        .iter()
        .chain(&layout.rows)
        .flatten()
        .filter(|&&n| n == clue_layout::BLOTTED)
        .count();
    if blotted > 0 {
        save_debug_image(Some(&states))?;
        anyhow::bail!(
            "{blotted} clues couldn't be read (shown as \"?\"), so there's no puzzle to write; \
             see --debug-image"
        );
    }
    let report = puzzle.plain_solve();
    match &report {
        Ok(report) if report.cells_left == 0 => eprintln!("Solvable with line logic."),
        Ok(report) => eprintln!("Line logic leaves {} cells unsolved.", report.cells_left),
        Err(_) => eprintln!("Warning: these clues contradict each other."),
    }
    // Where the grid disagrees with the answer, either the person made a mistake, or something
    // was misread.
    if let Ok(Report {
        solution: DynSolution::Square(answer),
        ..
    }) = &report
    {
        let mut wrong = 0;
        for (r, row) in states.iter().enumerate() {
            for (c, state) in row.iter().enumerate() {
                match (state, answer.get((c, r))) {
                    (cells::State::Filled, Some(color)) if color == BACKGROUND => wrong += 1,
                    (cells::State::Crossed, Some(color))
                        if color != BACKGROUND && color != UNSOLVED =>
                    {
                        wrong += 1
                    }
                    _ => {}
                }
            }
        }
        if wrong > 0 {
            eprintln!("Warning: {wrong} cells of the grid disagree with the answer.");
        }
    }
    save_debug_image(Some(&states))?;

    let output = args.output.unwrap_or_else(|| PathBuf::from("-"));
    let format = args
        .output_format
        .or_else(|| (output == Path::new("-")).then_some(NonogramFormat::Olsak));
    let mut document = Document::new(
        Some(DynPuzzle::SquareNono(puzzle)),
        None,
        output.display().to_string(),
        None,
        None,
        None,
        None,
        None,
    );
    export::save(&mut document, &output, format)
}

/// `--score`: read every picture in `dir`, and compare against what a human says is there.
fn score(engine: &OcrEngine, dir: &Path, args: &Args) -> anyhow::Result<()> {
    const PICTURES: &[&str] = &["webp", "png", "jpg", "jpeg", "gif", "bmp"];
    let mut pictures: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("reading {dir:?}"))?
        .map(|entry| entry.map(|e| e.path()))
        .collect::<Result<_, _>>()?;
    pictures.retain(|p| {
        p.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| PICTURES.contains(&e.to_lowercase().as_str()))
    });
    // Numerically, where the names are numbers.
    let stem = |p: &PathBuf| {
        p.file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string()
    };
    pictures.sort_by_key(|p| (stem(p).parse::<u64>().ok(), stem(p)));

    let mut total = Score::default();
    let (mut unchecked, mut written) = (vec![], vec![]);
    for picture in &pictures {
        let name = picture.file_name().unwrap_or_default().to_string_lossy();
        let reading = read(engine, picture, args)?;
        if let Some(dir) = &args.debug_dir {
            let path = dir.join(picture.with_extension("png").file_name().unwrap());
            debug_image(&reading, None)
                .save(&path)
                .with_context(|| format!("writing {path:?}"))?;
        }
        let (cols, rows) = match reading.layout {
            Ok(layout) => (layout.cols, layout.rows),
            Err(e) => {
                println!("{name}: couldn't read: {e:#}");
                (vec![], vec![])
            }
        };

        let clues_path = picture.with_extension("clues");
        if !clues_path.exists() {
            std::fs::write(&clues_path, Expected::render(&cols, &rows, &name))
                .with_context(|| format!("writing {clues_path:?}"))?;
            written.push(clues_path.display().to_string());
            continue;
        }
        let text = std::fs::read_to_string(&clues_path)
            .with_context(|| format!("reading {clues_path:?}"))?;
        let expected =
            Expected::parse(&text).with_context(|| format!("in {}", clues_path.display()))?;
        if !expected.checked {
            unchecked.push(clues_path.display().to_string());
            continue;
        }

        let score = expected.score(&cols, &rows);
        println!(
            "{name}: {}/{} lanes right, {}/{} numbers wrong",
            score.lanes_right, score.lanes, score.numbers_wrong, score.numbers
        );
        for mistake in &score.mistakes {
            println!("    {mistake}");
        }
        total.lanes += score.lanes;
        total.lanes_right += score.lanes_right;
        total.numbers += score.numbers;
        total.numbers_wrong += score.numbers_wrong;
    }

    if total.lanes > 0 {
        println!(
            "Total: {}/{} lanes right ({:.1}%), {}/{} numbers wrong ({:.1}%)",
            total.lanes_right,
            total.lanes,
            100.0 * total.lanes_right as f32 / total.lanes as f32,
            total.numbers_wrong,
            total.numbers,
            100.0 * total.numbers_wrong as f32 / total.numbers.max(1) as f32,
        );
    }
    if !written.is_empty() {
        println!(
            "Wrote what was read to these, for correcting (then delete the {} line):",
            Expected::UNCHECKED
        );
        for path in &written {
            println!("    {path}");
        }
    }
    if !unchecked.is_empty() {
        println!("Still {}, so not scored:", Expected::UNCHECKED);
        for path in &unchecked {
            println!("    {path}");
        }
    }
    Ok(())
}

fn debug_image(reading: &Reading, states: Option<&[Vec<cells::State>]>) -> image::RgbImage {
    use image::Rgb;
    let (image, glyphs, rejected) = (&reading.image, &reading.glyphs, &reading.rejected);
    let layout = reading.layout.as_ref().ok();
    let mut debug = image.clone();
    let (w, h) = (image.width() as f32, image.height() as f32);
    let orange = Rgb([255, 140, 0]);
    for r in rejected {
        draw::rect(&mut debug, r.rect, orange);
        draw::text(
            &mut debug,
            (r.rect.0, r.rect.3 + 2.0),
            12.0,
            &r.text,
            orange,
        );
    }
    if let Some(layout) = layout {
        let green = Rgb([0, 180, 0]);
        let (top, left) = (layout.grid_top, layout.grid_left);
        draw::line(&mut debug, (0.0, top.at(0.0)), (w, top.at(w)), green);
        draw::line(&mut debug, (left.at(0.0), 0.0), (left.at(h), h), green);
    }
    // The lines found, in purple, with the grid's bounds thicker.
    if let Some(grid) = &reading.grid {
        let purple = Rgb([160, 0, 220]);
        for (i, _) in grid.horizontal.intercepts.iter().enumerate() {
            let at = |x: f32| grid.horizontal.at(i, x);
            draw::line(&mut debug, (0.0, at(0.0)), (w, at(w)), purple);
            if i == grid.rows.0 || i == grid.rows.1 {
                draw::line(&mut debug, (0.0, at(0.0) + 2.0), (w, at(w) + 2.0), purple);
            }
        }
        for (i, _) in grid.vertical.intercepts.iter().enumerate() {
            let at = |y: f32| grid.vertical.at(i, y);
            draw::line(&mut debug, (at(0.0), 0.0), (at(h), h), purple);
            if i == grid.cols.0 || i == grid.cols.1 {
                draw::line(&mut debug, (at(0.0) + 2.0, 0.0), (at(h) + 2.0, h), purple);
            }
        }
    }
    // The cells' states: filled in green, crossed out in red, undecided in gray.
    if let (Some(states), Some(layout)) = (states, layout) {
        let r = 0.2 * layout.col_pitch.min(layout.row_pitch);
        for (row, &y) in states.iter().zip(&layout.row_centers) {
            for (state, &x) in row.iter().zip(&layout.col_centers) {
                match state {
                    cells::State::Filled => {
                        draw::rect(&mut debug, (x - r, y - r, x + r, y + r), Rgb([0, 200, 0]))
                    }
                    cells::State::Crossed => {
                        let red = Rgb([230, 0, 0]);
                        draw::line(&mut debug, (x - r, y - r), (x + r, y + r), red);
                        draw::line(&mut debug, (x - r, y + r), (x + r, y - r), red);
                    }
                    cells::State::Undecided => draw::rect(
                        &mut debug,
                        (x - 2.0, y - 2.0, x + 2.0, y + 2.0),
                        Rgb([150, 150, 150]),
                    ),
                }
            }
        }
    }
    // The second pass's numbers (to the right of the first's, so they don't overlap).
    for found in reading.reread.iter().flat_map(|r| &r.found) {
        let (left, top, right, bottom) = found.area;
        let area = (left as f32, top as f32, right as f32, bottom as f32);
        let (color, label) = match (found.number, found.recognized) {
            (Some(n), Some(was)) => (Rgb([220, 160, 0]), format!("{was}>{n}")),
            (Some(n), None) => (Rgb([0, 170, 170]), n.to_string()),
            (None, _) => (Rgb([255, 0, 255]), "?".to_string()),
        };
        draw::rect(&mut debug, area, color);
        draw::text(&mut debug, (area.2 + 3.0, area.1), 11.0, &label, color);
    }
    for (i, g) in glyphs.iter().enumerate() {
        let role = layout.map_or(Role::Ignored, |l| l.roles[i]);
        let (color, label) = match role {
            Role::Ignored => (Rgb([150, 150, 150]), String::new()),
            Role::Col(c) => (Rgb([0, 60, 255]), format!("c{}", c + 1)),
            Role::Row(r) => (Rgb([230, 0, 0]), format!("r{}", r + 1)),
        };
        let (hw, hh) = (g.width / 2.0, g.height / 2.0);
        draw::rect(&mut debug, (g.x - hw, g.y - hh, g.x + hw, g.y + hh), color);
        draw::text(
            &mut debug,
            (g.x - hw, g.y + hh + 1.0),
            11.0,
            &format!("{}{label}", g.digit),
            color,
        );
    }
    debug
}
