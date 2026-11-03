//! ScreenCaptureKit plumbing shared by screenshots and recording: waiting for its
//! completion blocks, permission errors and the list of displays.

use std::sync::mpsc;
use std::time::Duration;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::AnyThread;
use objc2_foundation::{NSArray, NSError};
use objc2_screen_capture_kit::{SCContentFilter, SCDisplay, SCShareableContent, SCWindow};

use super::pure;

/// ScreenCaptureKit answers within a moment; this only stops a lost answer from hanging us.
pub const ANSWER_TIMEOUT: Duration = Duration::from_secs(10);

/// SCStreamErrorUserDeclined.
const USER_DECLINED_CODE: isize = -3801;

pub fn describe_error(error: *mut NSError) -> String {
    let Some(error) = (unsafe { error.as_ref() }) else {
        return "ScreenCaptureKit gave no answer".into();
    };
    if error.code() == USER_DECLINED_CODE {
        return permission_error();
    }
    error.localizedDescription().to_string()
}

/// Opens the right System Settings pane and returns the message that says what to do there.
pub fn permission_error() -> String {
    let _ = std::process::Command::new("/usr/bin/open").arg(pure::SETTINGS_PANE_URL).spawn();
    pure::PERMISSION_MESSAGE.to_string()
}

/// Everything on screen that can be captured. Fails with the permission message when the
/// user has not allowed screen recording.
pub fn shareable_content() -> Result<Retained<SCShareableContent>, String> {
    if !objc2_core_graphics::CGPreflightScreenCaptureAccess() {
        // The first request shows the system prompt; the pane is for when it was declined before.
        objc2_core_graphics::CGRequestScreenCaptureAccess();
        return Err(permission_error());
    }
    let (sender, receiver) = mpsc::channel();
    let handler = RcBlock::new(move |content: *mut SCShareableContent, error: *mut NSError| {
        let result = unsafe { Retained::retain(content) }.ok_or_else(|| describe_error(error));
        let _ = sender.send(result);
    });
    unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&handler) };
    receiver.recv_timeout(ANSWER_TIMEOUT).map_err(|_| "ScreenCaptureKit did not answer".to_string())?
}

pub fn displays_of(content: &SCShareableContent) -> Vec<Retained<SCDisplay>> {
    unsafe { content.displays() }.iter().collect()
}

pub fn display_rect(display: &SCDisplay) -> pure::PointRect {
    let frame = unsafe { display.frame() };
    pure::PointRect {
        x: frame.origin.x,
        y: frame.origin.y,
        width: frame.size.width,
        height: frame.size.height,
    }
}

/// Physical pixels per point of one display (2.0 on Retina).
pub fn pixel_scale(display: &SCDisplay) -> f64 {
    let filter = unsafe {
        SCContentFilter::initWithDisplay_excludingWindows(SCContentFilter::alloc(), display, &NSArray::<SCWindow>::new())
    };
    f64::from(unsafe { SCShareableContent::infoForFilter(&filter).pointPixelScale() })
}
