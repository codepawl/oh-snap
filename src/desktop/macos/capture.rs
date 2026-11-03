//! Screenshots of every display and the list of windows.

use std::ffi::c_void;
use std::sync::mpsc;

use block2::RcBlock;
use objc2::AnyThread;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGColorSpace, CGContext, CGImage, CGImageAlphaInfo, CGImageByteOrderInfo,
    CGWindowListCopyWindowInfo, CGWindowListOption,
};
use objc2_foundation::{NSArray, NSDictionary, NSError, NSNumber, NSString};
use objc2_screen_capture_kit::{
    SCContentFilter, SCDisplay, SCScreenshotManager, SCStreamConfiguration, SCWindow,
};

use super::pure::{self, PointRect};
use super::sck;
use crate::desktop::platform::{Screen, WindowInfo};
use crate::output::Image;

/// 'BGRA' as a four character code.
const PIXEL_FORMAT_BGRA: u32 = 0x4247_5241;

/// A screenshot that has been asked for and not yet delivered.
struct Pending {
    receiver: mpsc::Receiver<Result<Retained<CGImage>, String>>,
    rect: PointRect,
    scale: f64,
}

/// Starts the capture of one display at its native pixel size.
fn request_capture(display: &SCDisplay) -> Pending {
    let rect = sck::display_rect(display);
    let filter = unsafe {
        SCContentFilter::initWithDisplay_excludingWindows(SCContentFilter::alloc(), display, &NSArray::<SCWindow>::new())
    };
    let scale = sck::pixel_scale(display);
    let configuration = unsafe { SCStreamConfiguration::new() };
    unsafe {
        configuration.setWidth((rect.width * scale).round() as usize);
        configuration.setHeight((rect.height * scale).round() as usize);
        configuration.setPixelFormat(PIXEL_FORMAT_BGRA);
        configuration.setShowsCursor(false);
    }
    let (sender, receiver) = mpsc::channel();
    let handler = RcBlock::new(move |image: *mut CGImage, error: *mut NSError| {
        let result = unsafe { Retained::retain(image) }.ok_or_else(|| sck::describe_error(error));
        let _ = sender.send(result);
    });
    unsafe {
        SCScreenshotManager::captureImageWithFilter_configuration_completionHandler(&filter, &configuration, Some(&handler));
    }
    Pending { receiver, rect, scale }
}

/// Draws the picture into our own BGRA buffer, whatever layout the system gave it.
fn image_to_bgra(picture: &CGImage) -> Result<Image, String> {
    let width = CGImage::width(Some(picture));
    let height = CGImage::height(Some(picture));
    let mut bgra = vec![0u8; width * height * 4];
    let color_space = CGColorSpace::new_device_rgb().ok_or("no color space")?;
    let layout = CGImageAlphaInfo::PremultipliedFirst.0 | CGImageByteOrderInfo::Order32Little.0;
    let context: objc2_core_foundation::CFRetained<CGContext> = unsafe {
        CGBitmapContextCreate(bgra.as_mut_ptr().cast::<c_void>(), width, height, 8, width * 4, Some(&color_space), layout)
    }
    .ok_or("could not make the picture buffer")?;
    let whole = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(width as f64, height as f64));
    CGContext::draw_image(Some(&context), whole, Some(picture));
    Ok(Image { width: width as u32, height: height as u32, bgra })
}

pub fn capture_screens() -> Result<Vec<Screen>, String> {
    let content = sck::shareable_content()?;
    let displays = sck::displays_of(&content);
    if displays.is_empty() {
        return Err("no display to capture".into());
    }
    // Ask for every display first so they are captured side by side, then collect.
    let pending: Vec<Pending> = displays.iter().map(|display| request_capture(display)).collect();
    let scales: Vec<f64> = pending.iter().map(|pending| pending.scale).collect();
    let desktop_scale = pure::desktop_scale(&scales);
    let mut screens = Vec::new();
    for pending in pending {
        let picture = pending
            .receiver
            .recv_timeout(sck::ANSWER_TIMEOUT)
            .map_err(|_| "the screenshot did not arrive".to_string())??;
        let position = pure::points_to_pixels(pending.rect, desktop_scale);
        screens.push(Screen {
            x: position.x,
            y: position.y,
            scale: pending.scale as f32,
            image: image_to_bgra(&picture)?,
        });
    }
    Ok(screens)
}

fn dictionary_number(dictionary: &NSDictionary<NSString, AnyObject>, key: &str) -> Option<f64> {
    let value = dictionary.objectForKey(&NSString::from_str(key))?;
    let number = value.downcast_ref::<NSNumber>()?;
    Some(number.doubleValue())
}

fn dictionary_text(dictionary: &NSDictionary<NSString, AnyObject>, key: &str) -> Option<String> {
    let value = dictionary.objectForKey(&NSString::from_str(key))?;
    let text = value.downcast_ref::<NSString>()?;
    Some(text.to_string())
}

fn window_info(entry: &NSDictionary<NSString, AnyObject>, own_process: f64, desktop_scale: f64) -> Option<WindowInfo> {
    if dictionary_number(entry, "kCGWindowLayer")? != 0.0 {
        return None;
    }
    if dictionary_number(entry, "kCGWindowOwnerPID") == Some(own_process) {
        return None;
    }
    if dictionary_number(entry, "kCGWindowAlpha").is_some_and(|alpha| alpha <= 0.0) {
        return None;
    }
    let bounds = entry.objectForKey(&NSString::from_str("kCGWindowBounds"))?;
    let bounds = bounds.downcast_ref::<NSDictionary>()?;
    let bounds: &NSDictionary<NSString, AnyObject> = unsafe { bounds.cast_unchecked() };
    let rect = PointRect {
        x: dictionary_number(bounds, "X")?,
        y: dictionary_number(bounds, "Y")?,
        width: dictionary_number(bounds, "Width")?,
        height: dictionary_number(bounds, "Height")?,
    };
    let pixels = pure::points_to_pixels(rect, desktop_scale);
    if pixels.width < 2 || pixels.height < 2 {
        return None;
    }
    // Window names need the screen recording permission; the owner's name is the fallback.
    let title = dictionary_text(entry, "kCGWindowName")
        .filter(|name| !name.is_empty())
        .or_else(|| dictionary_text(entry, "kCGWindowOwnerName"))
        .unwrap_or_default();
    Some(WindowInfo { x: pixels.x, y: pixels.y, width: pixels.width, height: pixels.height, title })
}

/// The largest backing scale among the attached displays, the same one `capture_screens` uses.
fn current_desktop_scale() -> f64 {
    let Ok(content) = sck::shareable_content() else {
        return 1.0;
    };
    let scales: Vec<f64> = sck::displays_of(&content).iter().map(|display| sck::pixel_scale(display)).collect();
    pure::desktop_scale(&scales)
}

pub fn windows_on_screen() -> Vec<WindowInfo> {
    let options = CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements;
    let Some(list) = CGWindowListCopyWindowInfo(options, 0) else {
        return Vec::new();
    };
    // A CFArray of CFDictionary is an NSArray of NSDictionary: the types are toll-free bridged.
    let list: Retained<NSArray<NSDictionary<NSString, AnyObject>>> =
        unsafe { Retained::from_raw(objc2_core_foundation::CFRetained::into_raw(list).as_ptr().cast()) }.expect("non-null list");
    let own_process = f64::from(std::process::id());
    let desktop_scale = current_desktop_scale();
    list.iter().filter_map(|entry| window_info(&entry, own_process, desktop_scale)).collect()
}
