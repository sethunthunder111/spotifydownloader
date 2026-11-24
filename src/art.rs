//! Turns cover art into coloured ASCII art for the terminal.

use anyhow::{Context, Result};
use image::RgbImage;
use image::imageops::FilterType;

/// Characters from empty to dense. Dense characters mean bright pixels, which suits dark terminals.
const RAMP: &[u8] = b" .:-=+*#%@";
/// Cover art is shrunk to this many pixels on its longest side before it is kept.
const KEEP_SIZE: u32 = 64;

/// A small copy of a cover image, ready to be drawn at any size.
#[derive(Clone)]
pub struct Cover {
    image: RgbImage,
}

/// One character cell of ASCII art and its colour.
#[derive(Clone, Copy)]
pub struct Cell {
    pub ch: char,
    pub rgb: [u8; 3],
}

impl Cover {
    /// Decodes JPEG or PNG bytes.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let image = image::load_from_memory(bytes)
            .context("cannot decode cover art")?
            .resize(KEEP_SIZE, KEEP_SIZE, FilterType::Triangle)
            .to_rgb8();
        Ok(Self { image })
    }

    /// Draws the cover as `cols` x `rows` characters. A terminal cell is about twice as tall as
    /// it is wide, so a square cover looks right when `cols` is about `rows * 2`.
    pub fn ascii(&self, cols: u16, rows: u16) -> Vec<Vec<Cell>> {
        if cols == 0 || rows == 0 {
            return Vec::new();
        }
        let small = image::imageops::resize(
            &self.image,
            u32::from(cols),
            u32::from(rows),
            FilterType::Triangle,
        );
        (0..rows)
            .map(|y| {
                (0..cols)
                    .map(|x| {
                        let [r, g, b] = small.get_pixel(u32::from(x), u32::from(y)).0;
                        Cell {
                            ch: ramp_char(r, g, b),
                            rgb: brighten([r, g, b]),
                        }
                    })
                    .collect()
            })
            .collect()
    }
}

/// The character for a pixel, chosen by perceived brightness.
fn ramp_char(r: u8, g: u8, b: u8) -> char {
    let luma = 0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b);
    let index = (luma / 255.0 * (RAMP.len() - 1) as f32).round() as usize;
    char::from(RAMP[index.min(RAMP.len() - 1)])
}

/// Lifts dark colours so thin characters stay visible on a dark background.
fn brighten([r, g, b]: [u8; 3]) -> [u8; 3] {
    let lift = |c: u8| (u16::from(c) + 60).min(255) as u8;
    [lift(r), lift(g), lift(b)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(rgb: [u8; 3]) -> Cover {
        Cover {
            image: RgbImage::from_pixel(8, 8, image::Rgb(rgb)),
        }
    }

    #[test]
    fn dark_pixels_get_sparse_characters_and_bright_ones_dense() {
        assert_eq!(solid([0, 0, 0]).ascii(2, 1)[0][0].ch, ' ');
        assert_eq!(solid([255, 255, 255]).ascii(2, 1)[0][0].ch, '@');
    }

    #[test]
    fn output_has_the_requested_size() {
        let art = solid([10, 120, 200]).ascii(6, 3);
        assert_eq!(art.len(), 3);
        assert!(art.iter().all(|row| row.len() == 6));
        assert!(solid([1, 2, 3]).ascii(0, 3).is_empty());
    }

    #[test]
    fn colours_are_lifted_but_capped() {
        let cell = solid([250, 0, 100]).ascii(1, 1)[0][0];
        assert_eq!(cell.rgb, [255, 60, 160]);
    }

    #[test]
    fn rejects_garbage_bytes() {
        assert!(Cover::decode(b"not an image").is_err());
    }
}
