//! Reads the text in a snip with the OCR engine that ships with Windows, for the user's
//! display languages. It runs in-process, so it takes no PowerShell start and no download.

use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Security::Cryptography::CryptographicBuffer;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};

use crate::output::Image;

/// Windows OCR misses small interface text at 100% scale; doubling it reads far better.
const UPSCALE_BELOW_HEIGHT: u32 = 600;

/// The text found in the image, one line per text line. Empty when there is none.
/// Blocks for a moment, so the tray process calls it off its window thread.
pub fn read_text(image: &Image) -> Result<String, String> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    recognize(image).map_err(|error| error.message())
}

fn recognize(image: &Image) -> windows::core::Result<String> {
    let enlarged = (image.height < UPSCALE_BELOW_HEIGHT).then(|| doubled(image));
    let source = enlarged.as_ref().unwrap_or(image);
    let buffer = CryptographicBuffer::CreateFromByteArray(&source.bgra)?;
    let bitmap = SoftwareBitmap::CreateCopyFromBuffer(&buffer, BitmapPixelFormat::Bgra8, source.width as i32, source.height as i32)?;
    let engine = OcrEngine::TryCreateFromUserProfileLanguages()?;
    let result = engine.RecognizeAsync(&bitmap)?.join()?;
    let mut lines = Vec::new();
    for line in result.Lines()? {
        lines.push(line.Text()?.to_string());
    }
    Ok(lines.join("\n"))
}

fn doubled(image: &Image) -> Image {
    let width = image.width as usize;
    let mut bgra = Vec::with_capacity(image.bgra.len() * 4);
    for row in image.bgra.chunks_exact(width * 4) {
        let mut wide_row = Vec::with_capacity(row.len() * 2);
        for pixel in row.chunks_exact(4) {
            wide_row.extend_from_slice(pixel);
            wide_row.extend_from_slice(pixel);
        }
        bgra.extend_from_slice(&wide_row);
        bgra.extend_from_slice(&wide_row);
    }
    Image {
        width: image.width * 2,
        height: image.height * 2,
        bgra,
    }
}

/// `oh-snap text FILE`: prints the text in a PNG.
pub fn command(path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let image = crate::image_file::load_png(std::path::Path::new(path))?;
    println!("{}", read_text(&image)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doubling_repeats_each_pixel() {
        let image = Image {
            width: 2,
            height: 1,
            bgra: vec![1, 2, 3, 255, 4, 5, 6, 255],
        };
        let enlarged = doubled(&image);
        assert_eq!((enlarged.width, enlarged.height), (4, 2));
        assert_eq!(&enlarged.bgra[..8], &[1, 2, 3, 255, 1, 2, 3, 255]);
        assert_eq!(&enlarged.bgra[16..20], &[1, 2, 3, 255]);
        assert_eq!(enlarged.bgra.len(), 32);
    }
}
