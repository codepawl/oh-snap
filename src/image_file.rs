//! Reads PNG files into the BGRA images the rest of oh-snap works with.

use std::path::Path;

use crate::output::Image;

pub fn load_png(path: &Path) -> Result<Image, Box<dyn std::error::Error>> {
    let file = std::fs::File::open(path)?;
    let mut decoder = png::Decoder::new(std::io::BufReader::new(file));
    // Palette, grey and 16-bit images all come out as 8-bit colour.
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info()?;
    let mut buffer = vec![0; reader.output_buffer_size().ok_or("PNG is too large")?];
    let frame = reader.next_frame(&mut buffer)?;
    let pixels = &buffer[..frame.buffer_size()];

    let pixel_count = frame.width as usize * frame.height as usize;
    let mut bgra = Vec::with_capacity(pixel_count * 4);
    match frame.color_type {
        png::ColorType::Rgba => {
            for pixel in pixels.chunks_exact(4) {
                // Transparent areas are shown over ink, the editor's background.
                let alpha = pixel[3] as u32;
                let blend = |channel: u8, ink: u32| ((channel as u32 * alpha + ink * (255 - alpha)) / 255) as u8;
                bgra.extend_from_slice(&[blend(pixel[2], 0x1D), blend(pixel[1], 0x1E), blend(pixel[0], 0x1F), 255]);
            }
        }
        png::ColorType::Rgb => {
            for pixel in pixels.chunks_exact(3) {
                bgra.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for pixel in pixels.chunks_exact(2) {
                bgra.extend_from_slice(&[pixel[0], pixel[0], pixel[0], 255]);
            }
        }
        png::ColorType::Grayscale => {
            for &grey in pixels {
                bgra.extend_from_slice(&[grey, grey, grey, 255]);
            }
        }
        png::ColorType::Indexed => return Err("indexed PNG was not expanded".into()),
    }
    Ok(Image {
        width: frame.width,
        height: frame.height,
        bgra,
    })
}
