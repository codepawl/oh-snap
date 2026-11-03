//! What happens to a snip: PNG and JPEG encode, save to the screenshots folder, colour readouts.
//! Windows adds crop and the clipboard in `clipboard`.

use std::path::{Path, PathBuf};

#[cfg(windows)]
pub use crate::clipboard::{copy_text_to_clipboard, copy_to_clipboard, crop};
use crate::clock::local_time;

/// A cropped BGRA image with opaque alpha.
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// Clears everything outside `outline`, which is in the image's pixels. What is cleared turns
/// white and see-through: PNG keeps it see-through, and formats without alpha show white.
pub fn keep_inside(image: &mut Image, outline: &[(f32, f32)]) {
    let width = image.width as usize;
    for row in 0..image.height as usize {
        let inside = spans_inside(outline, row as f32 + 0.5);
        for column in 0..width {
            let center = column as f32 + 0.5;
            if !inside.iter().any(|(left, right)| center >= *left && center < *right) {
                let offset = (row * width + column) * 4;
                image.bgra[offset..offset + 4].copy_from_slice(&[255, 255, 255, 0]);
            }
        }
    }
}

/// Where a horizontal line at `y` is inside the outline, by the even-odd rule.
fn spans_inside(outline: &[(f32, f32)], y: f32) -> Vec<(f32, f32)> {
    let mut crossings = Vec::new();
    for (index, from) in outline.iter().enumerate() {
        let to = outline[(index + 1) % outline.len()];
        if (from.1 <= y) != (to.1 <= y) {
            crossings.push(from.0 + (y - from.1) / (to.1 - from.1) * (to.0 - from.0));
        }
    }
    crossings.sort_by(f32::total_cmp);
    crossings.chunks_exact(2).map(|pair| (pair[0], pair[1])).collect()
}

pub fn encode_png(image: &Image) -> std::io::Result<Vec<u8>> {
    let has_clear_pixels = image.bgra.chunks_exact(4).any(|pixel| pixel[3] != 255);
    let channels = if has_clear_pixels { 4 } else { 3 };
    let mut rgb = Vec::with_capacity(image.width as usize * image.height as usize * channels);
    for pixel in image.bgra.chunks_exact(4) {
        rgb.push(pixel[2]);
        rgb.push(pixel[1]);
        rgb.push(pixel[0]);
        if has_clear_pixels {
            rgb.push(pixel[3]);
        }
    }
    let mut png_bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png_bytes, image.width, image.height);
        encoder.set_color(if has_clear_pixels { png::ColorType::Rgba } else { png::ColorType::Rgb });
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        let mut writer = encoder.write_header().map_err(std::io::Error::other)?;
        writer.write_image_data(&rgb).map_err(std::io::Error::other)?;
    }
    Ok(png_bytes)
}

pub use crate::folders::screenshots_folder;

/// Saves as "Screenshot 2026-10-29 143012.png", the name Snipping Tool uses.
pub fn save_png(png_bytes: &[u8], folder: &Path) -> std::io::Result<PathBuf> {
    save_file(png_bytes, folder, "png")
}

/// Saves encoded image bytes under the next free "Screenshot <time>" name.
/// "Screenshot 2026-10-30 011502", the name Windows gives its own screenshots.
pub fn screenshot_file_stem() -> String {
    let time = local_time();
    format!(
        "Screenshot {:04}-{:02}-{:02} {:02}{:02}{:02}",
        time.year, time.month, time.day, time.hour, time.minute, time.second
    )
}

pub fn save_file(bytes: &[u8], folder: &Path, extension: &str) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(folder)?;
    let stem = screenshot_file_stem();
    let mut path = folder.join(format!("{stem}.{extension}"));
    let mut copy_number = 2;
    while path.exists() {
        path = folder.join(format!("{stem} ({copy_number}).{extension}"));
        copy_number += 1;
    }
    std::fs::write(&path, bytes)?;
    Ok(path)
}

/// JPEG at quality 92: small files that still read well for text.
pub fn encode_jpeg(image: &Image) -> std::io::Result<Vec<u8>> {
    let mut jpeg_bytes = Vec::new();
    let encoder = jpeg_encoder::Encoder::new(&mut jpeg_bytes, 92);
    encoder
        .encode(&image.bgra, image.width as u16, image.height as u16, jpeg_encoder::ColorType::Bgra)
        .map_err(std::io::Error::other)?;
    Ok(jpeg_bytes)
}

pub fn write_png_to(png_bytes: &[u8], path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(path, png_bytes)
}

/// "#D97757" for a pixel's red, green and blue.
pub fn color_hex(rgb: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2])
}

/// "rgb(217, 119, 87)  hsl(15, 63%, 60%)" for the card.
pub fn color_details(rgb: [u8; 3]) -> String {
    let (hue, saturation, lightness) = hsl(rgb);
    format!("rgb({}, {}, {})  hsl({hue}, {saturation}%, {lightness}%)", rgb[0], rgb[1], rgb[2])
}

/// Hue in degrees, saturation and lightness in percent, rounded.
fn hsl(rgb: [u8; 3]) -> (u32, u32, u32) {
    let red = f32::from(rgb[0]) / 255.0;
    let green = f32::from(rgb[1]) / 255.0;
    let blue = f32::from(rgb[2]) / 255.0;
    let largest = red.max(green).max(blue);
    let smallest = red.min(green).min(blue);
    let range = largest - smallest;
    let lightness = (largest + smallest) / 2.0;
    if range == 0.0 {
        return (0, 0, (lightness * 100.0).round() as u32);
    }
    let saturation = range / (1.0 - (2.0 * lightness - 1.0).abs());
    let sector = if largest == red {
        ((green - blue) / range).rem_euclid(6.0)
    } else if largest == green {
        (blue - red) / range + 2.0
    } else {
        (red - green) / range + 4.0
    };
    let hue = (sector * 60.0).round() as u32 % 360;
    (hue, (saturation * 100.0).round() as u32, (lightness * 100.0).round() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn white_square(side: u32) -> Image {
        Image {
            width: side,
            height: side,
            bgra: vec![200; (side * side * 4) as usize],
        }
    }

    #[test]
    fn only_the_inside_of_an_outline_stays() {
        let mut image = white_square(10);
        keep_inside(&mut image, &[(2.0, 2.0), (8.0, 2.0), (8.0, 8.0), (2.0, 8.0)]);
        let alpha = |x: usize, y: usize| image.bgra[(y * 10 + x) * 4 + 3];
        assert_eq!(alpha(5, 5), 200);
        assert_eq!(alpha(2, 2), 200);
        assert_eq!(alpha(1, 5), 0);
        assert_eq!(alpha(8, 5), 0);
        assert_eq!(alpha(5, 9), 0);
        assert_eq!(&image.bgra[..4], &[255, 255, 255, 0]);
    }

    #[test]
    fn a_crossed_outline_follows_the_even_odd_rule() {
        // A bow tie: the two triangles are inside, the gaps at the top and bottom middle are not.
        let spans = spans_inside(&[(0.0, 0.0), (10.0, 10.0), (10.0, 0.0), (0.0, 10.0)], 2.0);
        assert_eq!(spans.len(), 2);
    }

    #[test]
    fn png_keeps_clear_pixels_only_when_there_are_some() {
        let opaque = Image { width: 1, height: 1, bgra: vec![1, 2, 3, 255] };
        let clear = Image { width: 1, height: 1, bgra: vec![255, 255, 255, 0] };
        let color_type = |bytes: Vec<u8>| bytes[25];
        assert_eq!(color_type(encode_png(&opaque).unwrap()), 2);
        assert_eq!(color_type(encode_png(&clear).unwrap()), 6);
    }

    #[test]
    fn colors_read_as_hex_and_hsl() {
        assert_eq!(color_hex([0xD9, 0x77, 0x57]), "#D97757");
        assert_eq!(hsl([0xD9, 0x77, 0x57]), (15, 63, 60));
        assert_eq!(hsl([255, 255, 255]), (0, 0, 100));
        assert_eq!(hsl([0, 0, 255]), (240, 100, 50));
        assert_eq!(hsl([255, 0, 128]), (330, 100, 50));
    }
}
