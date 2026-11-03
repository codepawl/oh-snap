//! Our BGRA `Image` as a CoreGraphics picture, for the pasteboard, Vision and the GIF writer.

use std::ffi::c_void;

use objc2_core_foundation::CFRetained;
use objc2_core_graphics::{CGBitmapContextCreate, CGBitmapContextCreateImage, CGColorSpace, CGImage, CGImageAlphaInfo, CGImageByteOrderInfo};

use crate::output::Image;

pub fn cgimage_from(image: &Image) -> Result<CFRetained<CGImage>, String> {
    let color_space = CGColorSpace::new_device_rgb().ok_or("no color space")?;
    let layout = CGImageAlphaInfo::NoneSkipFirst.0 | CGImageByteOrderInfo::Order32Little.0;
    // The context copies nothing until the picture is made, and the picture copies the pixels.
    let mut pixels = image.bgra.clone();
    let context = unsafe {
        CGBitmapContextCreate(
            pixels.as_mut_ptr().cast::<c_void>(),
            image.width as usize,
            image.height as usize,
            8,
            image.width as usize * 4,
            Some(&color_space),
            layout,
        )
    }
    .ok_or("could not make the picture buffer")?;
    CGBitmapContextCreateImage(Some(&context)).ok_or_else(|| "could not make the picture".to_string())
}
