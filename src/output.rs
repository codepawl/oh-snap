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

pub fn encode_png(image: &Image) -> std::io::Result<Vec<u8>> {
    let mut rgb = Vec::with_capacity(image.width as usize * image.height as usize * 3);
    for pixel in image.bgra.chunks_exact(4) {
        rgb.push(pixel[2]);
        rgb.push(pixel[1]);
        rgb.push(pixel[0]);
    }
    let mut png_bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png_bytes, image.width, image.height);
        encoder.set_color(png::ColorType::Rgb);
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
