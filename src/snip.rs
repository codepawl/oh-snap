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
    pub particles: bool,
}

pub struct SnipOutcome {
    pub area: PixelRect,
    pub image: Image,
    pub copied: bool,
    pub saved_to: Option<PathBuf>,
}

pub type SnipResult = Result<Option<SnipOutcome>, Box<dyn std::error::Error>>;

/// Captures the screen and shows the overlay; returns the picked area, or None on cancel.
pub fn pick(capturer: &mut Capturer, graphics: &Graphics, request: &SnipRequest) -> Result<Option<PixelRect>, Box<dyn std::error::Error>> {
    let screenshot = capturer.capture()?;
    let options = OverlayOptions {
        start_mode: request.mode,
        close_after_first_frame: false,
        particles: request.particles,
    };
    Ok(overlay::select_area(graphics, screenshot, options)?.selection)
}

/// Pick and deliver in one go, for the CLI; the tray process adds the confirm step.
pub fn run(capturer: &mut Capturer, graphics: &Graphics, owner: Option<HWND>, request: &SnipRequest) -> SnipResult {
    let Some(area) = pick(capturer, graphics, request)? else {
        return Ok(None);
    };
    deliver(capturer, owner, request, area).map(Some)
}

/// Captures a known area without any UI (the CLI's `--rect`).
pub fn run_area(capturer: &mut Capturer, owner: Option<HWND>, request: &SnipRequest, area: PixelRect) -> SnipResult {
    capturer.capture()?;
    let quiet = SnipRequest {
        mode: request.mode,
        clipboard: request.clipboard,
        save_to_folder: request.save_to_folder,
        output_path: request.output_path.clone(),
        shutter_sound: false,
        particles: false,
    };
    deliver(capturer, owner, &quiet, area).map(Some)
}

/// The picked area as an image, from the frame the overlay showed.
pub fn crop(capturer: &Capturer, area: PixelRect) -> Result<Image, Box<dyn std::error::Error>> {
    Ok(output::crop(capturer.screenshot(), area).ok_or("the selected area is off screen")?)
}

/// Clipboard and file for an area of the last capture, with the shutter sound.
pub fn deliver(
    capturer: &Capturer,
    owner: Option<HWND>,
    request: &SnipRequest,
    area: PixelRect,
) -> Result<SnipOutcome, Box<dyn std::error::Error>> {
    let started = Instant::now();
    if request.shutter_sound {
        sound::play_shutter();
    }
    let image = crop(capturer, area)?;
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
