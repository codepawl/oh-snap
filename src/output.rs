//! What happens to a snip: crop, PNG encode, save to Pictures\Screenshots, clipboard.

use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
use windows::Win32::Graphics::Gdi::{BI_RGB, BITMAPINFOHEADER};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::{CF_DIB, CF_UNICODETEXT};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::UI::Shell::{FOLDERID_Screenshots, KF_FLAG_CREATE, SHGetKnownFolderPath};
use windows::core::{Result, w};

use crate::capture::{PixelRect, Screenshot};

/// A cropped BGRA image with opaque alpha.
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

pub fn crop(screenshot: &Screenshot, area: PixelRect) -> Option<Image> {
    let area = area.intersect(&screenshot.bounds)?;
    let stride = screenshot.stride();
    let row_bytes = area.width as usize * 4;
    let mut bgra = Vec::with_capacity(row_bytes * area.height as usize);
    let first_column = (area.left - screenshot.bounds.left) as usize;
    let first_row = (area.top - screenshot.bounds.top) as usize;
    for row in first_row..first_row + area.height as usize {
        let start = row * stride + first_column * 4;
        bgra.extend_from_slice(&screenshot.pixels[start..start + row_bytes]);
    }
    Some(Image {
        width: area.width as u32,
        height: area.height as u32,
        bgra,
    })
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

/// The folder Windows itself uses for screenshots, so Photos and Explorer list our files too.
pub fn screenshots_folder() -> PathBuf {
    let known = unsafe { SHGetKnownFolderPath(&FOLDERID_Screenshots, KF_FLAG_CREATE, None) };
    if let Ok(raw_path) = known {
        let path = unsafe { raw_path.to_string() }.unwrap_or_default();
        unsafe { CoTaskMemFree(Some(raw_path.0 as *const _)) };
        if !path.is_empty() {
            return PathBuf::from(path);
        }
    }
    let profile = std::env::var("USERPROFILE").unwrap_or_else(|_| ".".into());
    Path::new(&profile).join("Pictures").join("Screenshots")
}

/// Saves as "Screenshot 2026-10-29 143012.png", the name Snipping Tool uses.
pub fn save_png(png_bytes: &[u8], folder: &Path) -> std::io::Result<PathBuf> {
    save_file(png_bytes, folder, "png")
}

/// Saves encoded image bytes under the next free "Screenshot <time>" name.
/// "Screenshot 2026-10-30 011502", the name Windows gives its own screenshots.
pub fn screenshot_file_stem() -> String {
    let time = unsafe { GetLocalTime() };
    format!(
        "Screenshot {:04}-{:02}-{:02} {:02}{:02}{:02}",
        time.wYear, time.wMonth, time.wDay, time.wHour, time.wMinute, time.wSecond
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

/// Puts the image on the clipboard as a DIB (every app reads it) and as PNG (apps that
/// prefer it, such as browsers and chat apps, get the exact pixels).
pub fn copy_to_clipboard(owner: Option<HWND>, image: &Image, png_bytes: &[u8]) -> Result<()> {
    let dib = build_dib(image);
    open_clipboard_with_retry(owner)?;
    let result = unsafe { fill_clipboard(&dib, png_bytes) };
    unsafe { CloseClipboard()? };
    result
}

unsafe fn fill_clipboard(dib: &[u8], png_bytes: &[u8]) -> Result<()> {
    unsafe {
        EmptyClipboard()?;
        set_clipboard_bytes(CF_DIB.0 as u32, dib)?;
        let png_format = RegisterClipboardFormatW(w!("PNG"));
        if png_format != 0 {
            set_clipboard_bytes(png_format, png_bytes)?;
        }
    }
    Ok(())
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

/// Puts plain text on the clipboard, for add-ons such as OCR.
pub fn copy_text_to_clipboard(owner: Option<HWND>, text: &str) -> Result<()> {
    let utf16: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = unsafe { std::slice::from_raw_parts(utf16.as_ptr() as *const u8, utf16.len() * 2) };
    open_clipboard_with_retry(owner)?;
    let result = unsafe {
        EmptyClipboard().and_then(|()| set_clipboard_bytes(CF_UNICODETEXT.0 as u32, bytes))
    };
    unsafe { CloseClipboard()? };
    result
}

fn open_clipboard_with_retry(owner: Option<HWND>) -> Result<()> {
    let mut last_error = None;
    for _ in 0..10 {
        match unsafe { OpenClipboard(owner) } {
            Ok(()) => return Ok(()),
            Err(error) => last_error = Some(error),
        }
        // Another app may hold the clipboard for a moment (clipboard managers do).
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    Err(last_error.expect("loop ran at least once"))
}

unsafe fn set_clipboard_bytes(format: u32, bytes: &[u8]) -> Result<()> {
    unsafe {
        let memory: HGLOBAL = GlobalAlloc(GMEM_MOVEABLE, bytes.len())?;
        let destination = GlobalLock(memory) as *mut u8;
        if destination.is_null() {
            let _ = GlobalFree(Some(memory));
            return Err(windows::core::Error::from_thread());
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), destination, bytes.len());
        let _ = GlobalUnlock(memory);
        if let Err(error) = SetClipboardData(format, Some(HANDLE(memory.0))) {
            let _ = GlobalFree(Some(memory));
            return Err(error);
        }
    }
    Ok(())
}

/// A bottom-up 32-bit DIB: BITMAPINFOHEADER followed by the rows, last row first.
fn build_dib(image: &Image) -> Vec<u8> {
    let header = BITMAPINFOHEADER {
        biSize: size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: image.width as i32,
        biHeight: image.height as i32,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB.0,
        biSizeImage: image.width * image.height * 4,
        ..Default::default()
    };
    let header_bytes = unsafe {
        std::slice::from_raw_parts(&header as *const BITMAPINFOHEADER as *const u8, size_of::<BITMAPINFOHEADER>())
    };
    let row_bytes = image.width as usize * 4;
    let mut dib = Vec::with_capacity(header_bytes.len() + image.bgra.len());
    dib.extend_from_slice(header_bytes);
    for row in image.bgra.chunks_exact(row_bytes).rev() {
        dib.extend_from_slice(row);
    }
    dib
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
