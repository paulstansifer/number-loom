use anyhow::Context;
use std::path::PathBuf;

use crate::{
    formats::{
        char_grid::as_char_grid,
        html::as_html,
        image::as_image_bytes,
        olsak::{as_olsak_nono, as_olsak_triano},
        webpbn::as_webpbn,
        woven::to_woven,
    },
    geometry::{Shape, Square},
    puzzle::{self, DynPuzzle, NonogramFormat, Solution, UncertainDocument},
};

/// The square-only writers need a square picture; asking for one is how we find out.
fn square_solution(document: &mut UncertainDocument) -> anyhow::Result<&Solution<Square>> {
    document
        .solution()?
        .as_square()
        .context("this format needs a square puzzle, not a triddler")
}

pub fn to_bytes(
    document: &mut UncertainDocument,
    file_name: Option<String>,
    format: Option<NonogramFormat>,
) -> anyhow::Result<Vec<u8>> {
    let format = match format {
        Some(format) => format,
        None => puzzle::infer_format(
            file_name
                .as_ref()
                .context("no file format specified, or a filename to infer it from")?,
            None,
        ),
    };

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
        let file_name = file_name.context("need a filename to infer an image format")?;
        as_image_bytes(square_solution(document)?, file_name)?
    } else {
        match format {
            NonogramFormat::Olsak => match document.puzzle() {
                DynPuzzle::SquareNono(p) => as_olsak_nono(p)?,
                DynPuzzle::TriNono(p) => as_olsak_nono(p)?,
                DynPuzzle::SquareTriano(p) => as_olsak_triano(p)?,
            },
            NonogramFormat::Webpbn => as_webpbn(document)?,
            NonogramFormat::Html => {
                let (title, author) = (document.title.clone(), document.author.clone());
                crate::with_puzzle!(document.puzzle(), |p| as_html(p, &title, &author))
            }
            NonogramFormat::Image => unreachable!("handled above"),
            NonogramFormat::Woven => to_woven(&document.clone().into_document()?)?,
            NonogramFormat::CharGrid => as_char_grid(square_solution(document)?)?,
        }
        .into_bytes()
    };

    Ok(bytes)
}

pub fn save(
    document: &mut UncertainDocument,
    path: &PathBuf,
    format: Option<NonogramFormat>,
) -> anyhow::Result<()> {
    let bytes = to_bytes(document, Some(path.to_string_lossy().into_owned()), format)?;

    if path == &PathBuf::from("-") {
        use std::io::Write;
        std::io::stdout().write_all(&bytes)?;
        std::io::stdout().flush()?;
    } else {
        std::fs::write(path, bytes)?
    }
    Ok(())
}
