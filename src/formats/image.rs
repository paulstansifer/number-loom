use std::collections::HashMap;
use std::path::Path;

use image::{DynamicImage, GenericImageView, ImageFormat, Pixel, Rgb, RgbImage, Rgba};

use crate::puzzle::{ClueStyle, Color, ColorInfo, Solution, BACKGROUND};

pub fn image_to_solution(image: &DynamicImage) -> Solution {
    let (width, height) = image.dimensions();

    let mut palette = HashMap::<image::Rgba<u8>, ColorInfo>::new();
    let mut grid: Vec<Vec<Color>> = vec![vec![BACKGROUND; height as usize]; width as usize];

    // pbnsolve output looks weird if the default color isn't called "white".
    palette.insert(
        image::Rgba::<u8>([255, 255, 255, 255]),
        ColorInfo::default_bg(),
    );

    let mut next_char = 'a';
    let mut next_color_idx: u8 = 1; // BACKGROUND is 0

    // Gather the palette
    for y in 0..height {
        for x in 0..width {
            let pixel: Rgba<u8> = image.get_pixel(x, y);
            let color = palette.entry(pixel).or_insert_with(|| {
                let this_char = next_char;
                let [r, g, b] = pixel.channels()[0..3] else {
                    panic!("Image with fewer than three channels?")
                };
                let this_color = Color(next_color_idx);

                // Don't crash for too many colors, but the quality check should complain:
                next_color_idx = next_color_idx.wrapping_add(1);

                if r == 0 && g == 0 && b == 0 {
                    return ColorInfo::default_fg(this_color);
                }

                next_char = (next_char as u8).wrapping_add(1) as char;

                ColorInfo {
                    ch: this_char,
                    name: format!("{}{}", this_char, format!("{:02X}{:02X}{:02X}", r, g, b)),
                    rgb: (r, g, b),
                    color: this_color,
                    corner: None,
                }
            });

            grid[x as usize][y as usize] = color.color;
        }
    }

    Solution {
        clue_style: ClueStyle::Nono, // Images can't have triangular pixels!
        palette: palette
            .into_values()
            .map(|color_info| (color_info.color, color_info))
            .collect(),
        grid,
    }
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
