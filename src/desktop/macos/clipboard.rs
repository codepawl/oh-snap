//! The pasteboard: images as PNG and TIFF, and plain text.

use objc2::AnyThread;
use objc2_app_kit::{NSImage, NSPasteboard, NSPasteboardTypePNG, NSPasteboardTypeString, NSPasteboardTypeTIFF};
use objc2_foundation::{NSData, NSSize, NSString};

use super::cgimage::cgimage_from;
use crate::output::Image;

pub fn copy_image(image: &Image, png: &[u8]) -> Result<(), String> {
    let picture = cgimage_from(image)?;
    let native = NSImage::initWithCGImage_size(NSImage::alloc(), &picture, NSSize::new(0.0, 0.0));
    let tiff = native.TIFFRepresentation();
    let pasteboard = NSPasteboard::generalPasteboard();
    pasteboard.clearContents();
    let mut copied = unsafe { pasteboard.setData_forType(Some(&NSData::with_bytes(png)), NSPasteboardTypePNG) };
    if let Some(tiff) = tiff {
        copied |= unsafe { pasteboard.setData_forType(Some(&tiff), NSPasteboardTypeTIFF) };
    }
    if copied { Ok(()) } else { Err("the clipboard refused the picture".into()) }
}

pub fn copy_text(text: &str) -> Result<(), String> {
    let pasteboard = NSPasteboard::generalPasteboard();
    pasteboard.clearContents();
    let copied = unsafe { pasteboard.setString_forType(&NSString::from_str(text), NSPasteboardTypeString) };
    if copied { Ok(()) } else { Err("the clipboard refused the text".into()) }
}
