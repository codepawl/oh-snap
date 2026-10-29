//! One snip from start to finish: capture, pick an area, then clipboard and file.

use std::path::PathBuf;
use std::time::Instant;

use windows::Win32::Foundation::HWND;

use crate::capture::{Capturer, PixelRect};
use crate::output::{self, Image};
use crate::overlay::{self, Graphics, Mode, OverlayOptions};
use crate::sound;

pub struct SnipRequest {
    pub mode: Mode,
    pub clipboard: bool,
    pub save_to_folder: bool,
    pub output_path: Option<PathBuf>,
    pub shutter_sound: bool,
}

pub struct SnipOutcome {
    pub area: PixelRect,
    pub image: Image,
    pub copied: bool,
    pub saved_to: Option<PathBuf>,
}

pub type SnipResult = Result<Option<SnipOutcome>, Box<dyn std::error::Error>>;

/// Shows the overlay; returns `None` when the user cancels.
pub fn run(capturer: &mut Capturer, graphics: &Graphics, owner: Option<HWND>, request: &SnipRequest) -> SnipResult {
    let screenshot = capturer.capture()?;
    let options = OverlayOptions {
        start_mode: request.mode,
        close_after_first_frame: false,
    };
    let outcome = overlay::select_area(graphics, screenshot, options)?;
    let Some(area) = outcome.selection else {
        return Ok(None);
    };
    if request.shutter_sound {
        sound::play_shutter();
    }
    deliver(capturer, owner, request, area).map(Some)
}

/// Captures a known area without any UI (the CLI's `--rect`).
pub fn run_area(capturer: &mut Capturer, owner: Option<HWND>, request: &SnipRequest, area: PixelRect) -> SnipResult {
    capturer.capture()?;
    deliver(capturer, owner, request, area).map(Some)
}

fn deliver(
    capturer: &Capturer,
    owner: Option<HWND>,
    request: &SnipRequest,
    area: PixelRect,
) -> Result<SnipOutcome, Box<dyn std::error::Error>> {
    let started = Instant::now();
    let image = output::crop(capturer.screenshot(), area).ok_or("the selected area is off screen")?;
    let png_bytes = output::encode_png(&image)?;
    if request.clipboard {
        output::copy_to_clipboard(owner, &image, &png_bytes)?;
    }
    let mut saved_to = None;
    if let Some(path) = &request.output_path {
        output::write_png_to(&png_bytes, path)?;
        saved_to = Some(path.clone());
    } else if request.save_to_folder {
        saved_to = Some(output::save_png(&png_bytes, &output::screenshots_folder())?);
    }
    crate::log::write(&format!(
        "snip {}x{} at {},{} delivered in {} ms",
        area.width,
        area.height,
        area.left,
        area.top,
        started.elapsed().as_millis()
    ));
    Ok(SnipOutcome {
        area,
        image,
        copied: request.clipboard,
        saved_to,
    })
}
